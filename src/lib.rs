//! Developer live-integration spike for Adaptive ETA.
//!
//! The SCS-facing layer assembles one raw snapshot per frame and immediately
//! routes every lifecycle/frame input through the same deterministic adapter
//! and calibration pipeline used by offline replay. An opt-in developer marker
//! enables schema-v1 JSONL capture plus a structured live decision trace.

mod bridge;
mod capture;

use bridge::{BridgeDiagnostic, ChannelKind, FrameStart, LiveFrameAssembler};
use capture::DeveloperCapture;
use scs_sdk_plugin::sdk::{
    ChannelFlags, TelemetryApiVersion, channels, configuration, game, gameplay,
};
use scs_sdk_plugin::{
    ChannelUpdate, Game, GameCompatibility, PluginCompatibility, PluginContext, PluginMetadata,
    PluginResult, TelemetryEvent, TelemetryEventKind, TelemetryPlugin, export_plugin,
};
use telemetry_adapter::{AdapterOutput, DeterministicPipeline, RawInput};

static SUPPORTED_GAMES: [GameCompatibility; 1] = [GameCompatibility::new(
    Game::EuroTruckSimulator2,
    game::ets2::V1_12,
)];

#[derive(Debug)]
struct TelemetrySpike {
    assembler: LiveFrameAssembler,
    pipeline: DeterministicPipeline,
    capture: Option<DeveloperCapture>,
    callback_ordinal: u64,
    input_ordinal: usize,
    capture_failure_reported: bool,
    callback_trace_failure_reported: bool,
    paused: bool,
}

impl Default for TelemetrySpike {
    fn default() -> Self {
        Self {
            assembler: LiveFrameAssembler::default(),
            pipeline: DeterministicPipeline::default(),
            capture: None,
            callback_ordinal: 0,
            input_ordinal: 0,
            capture_failure_reported: false,
            callback_trace_failure_reported: false,
            paused: true,
        }
    }
}

impl TelemetrySpike {
    fn process_input(&mut self, context: &PluginContext<'_>, input: RawInput) {
        if let Some(capture) = &mut self.capture
            && let Err(error) = capture.record_input(input)
            && !self.capture_failure_reported
        {
            self.capture_failure_reported = true;
            context.error(format_args!(
                "[adaptive-eta-live] diagnostic kind=captureWriteFailure error={error}"
            ));
        }

        let step = self.pipeline.process(self.input_ordinal, input);
        self.input_ordinal += 1;
        for output in &step.adapter_outputs {
            if let AdapterOutput::Diagnostic(diagnostic) = output {
                context.warning(format_args!(
                    "[adaptive-eta-live] adapterDiagnostic input={} detail={:?}",
                    step.record_index + 1,
                    diagnostic
                ));
            }
        }
        for output in &step.core_outputs {
            context.message(format_args!(
                "[adaptive-eta-live] coreDecision input={} detail={output:?}",
                step.record_index + 1
            ));
        }
        if let Some(capture) = &mut self.capture {
            capture.record_step(step);
        }
    }

    fn trace_callback(&mut self, context: &PluginContext<'_>, detail: &str) {
        self.callback_ordinal = self.callback_ordinal.wrapping_add(1);
        if let Some(capture) = &mut self.capture
            && let Err(error) = capture.trace_callback(self.callback_ordinal, detail)
            && !self.callback_trace_failure_reported
        {
            self.callback_trace_failure_reported = true;
            context.warning(format_args!(
                "[adaptive-eta-live] diagnostic kind=callbackTraceWriteFailure error={error}"
            ));
        }
    }

    fn callback_trace_enabled(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(DeveloperCapture::callback_trace_enabled)
    }

    fn bridge_diagnostic(context: &PluginContext<'_>, diagnostic: BridgeDiagnostic) {
        context.warning(format_args!(
            "[adaptive-eta-live] bridgeDiagnostic detail={diagnostic:?}"
        ));
    }

    fn update_channel(&mut self, context: &PluginContext<'_>, update: ChannelUpdate<'_>) {
        let (channel_name, value_text, diagnostic) = if update.is(channels::common::LOCAL_SCALE) {
            let value = update.value(channels::common::LOCAL_SCALE).map(f64::from);
            (
                "local.scale",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler.channel_f64(ChannelKind::LocalScale, value),
            )
        } else if update.is(channels::common::GAME_TIME) {
            let value = update.value(channels::common::GAME_TIME);
            (
                "game.time",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler.channel_game_time(value),
            )
        } else if update.is(channels::truck::SPEED) {
            let value = update.value(channels::truck::SPEED).map(f64::from);
            (
                "truck.speed",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler.channel_f64(ChannelKind::Speed, value),
            )
        } else if update.is(channels::truck::ODOMETER) {
            let value = update.value(channels::truck::ODOMETER).map(f64::from);
            (
                "truck.odometer",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler.channel_f64(ChannelKind::Odometer, value),
            )
        } else if update.is(channels::truck::NAVIGATION_DISTANCE) {
            let value = update
                .value(channels::truck::NAVIGATION_DISTANCE)
                .map(f64::from);
            (
                "truck.navigation.distance",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler
                    .channel_f64(ChannelKind::NavigationDistance, value),
            )
        } else if update.is(channels::truck::NAVIGATION_TIME) {
            let value = update
                .value(channels::truck::NAVIGATION_TIME)
                .map(f64::from);
            (
                "truck.navigation.time",
                self.callback_trace_enabled().then(|| format!("{value:?}")),
                self.assembler
                    .channel_f64(ChannelKind::NavigationTime, value),
            )
        } else {
            return;
        };
        if let Some(value_text) = value_text {
            self.trace_callback(context, &format!("channel {channel_name}={value_text}"));
        } else {
            self.callback_ordinal = self.callback_ordinal.wrapping_add(1);
        }
        if let Some(diagnostic) = diagnostic {
            Self::bridge_diagnostic(context, diagnostic);
        }
    }

    fn finish_capture(&mut self, context: &PluginContext<'_>) {
        let report = self.pipeline.report(Vec::new());
        let Some(capture) = self.capture.take() else {
            return;
        };
        match capture.finish(report) {
            Ok(paths) => context.message(format_args!(
                "[adaptive-eta-live] captureClosed capture={} liveTrace={} valid=true",
                paths.capture.display(),
                paths.live_trace.display()
            )),
            Err(error) => context.error(format_args!(
                "[adaptive-eta-live] diagnostic kind=captureInvalidated error={error}"
            )),
        }
    }
}

impl TelemetryPlugin for TelemetrySpike {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "Adaptive ETA Live Integration Spike",
            env!("CARGO_PKG_VERSION"),
        )
    }

    fn compatibility(&self) -> PluginCompatibility {
        PluginCompatibility::new(TelemetryApiVersion::V1_01, &SUPPORTED_GAMES)
    }

    fn initialize(&mut self, context: &mut PluginContext<'_>) -> PluginResult {
        self.pipeline = DeterministicPipeline::default();
        self.callback_ordinal = 0;
        self.input_ordinal = 0;
        self.capture_failure_reported = false;
        self.callback_trace_failure_reported = false;
        self.paused = true;
        self.capture = match DeveloperCapture::open_if_enabled() {
            Ok(capture) => capture,
            Err(error) => {
                context.error(format_args!(
                    "[adaptive-eta-live] diagnostic kind=captureOpenFailure error={error}"
                ));
                None
            }
        };

        context.subscribe_event(TelemetryEventKind::FrameStart)?;
        context.subscribe_event(TelemetryEventKind::FrameEnd)?;
        context.subscribe_event(TelemetryEventKind::Paused)?;
        context.subscribe_event(TelemetryEventKind::Started)?;
        context.subscribe_event(TelemetryEventKind::Configuration)?;
        context.subscribe_event(TelemetryEventKind::Gameplay)?;

        let flags = ChannelFlags::EACH_FRAME | ChannelFlags::NO_VALUE;
        context.subscribe_with_flags(channels::common::LOCAL_SCALE, flags)?;
        context.subscribe_with_flags(channels::common::GAME_TIME, flags)?;
        context.subscribe_with_flags(channels::truck::SPEED, flags)?;
        context.subscribe_with_flags(channels::truck::ODOMETER, flags)?;
        context.subscribe_with_flags(channels::truck::NAVIGATION_DISTANCE, flags)?;
        context.subscribe_with_flags(channels::truck::NAVIGATION_TIME, flags)?;

        if let Some(capture) = &self.capture {
            let paths = capture.paths();
            context.message(format_args!(
                "[adaptive-eta-live] captureOpened capture={} liveTrace={} callbackTrace={}",
                paths.capture.display(),
                paths.live_trace.display(),
                paths
                    .callback_trace
                    .as_ref()
                    .map_or_else(|| "disabled".to_owned(), |path| path.display().to_string())
            ));
        } else {
            context.message(format_args!(
                "[adaptive-eta-live] diagnostic kind=captureDisabled"
            ));
        }
        let connected = self.assembler.source_connected();
        self.process_input(context, connected);
        context.message(format_args!("[adaptive-eta-live] initialized"));
        Ok(())
    }

    fn channel(&mut self, context: &mut PluginContext<'_>, update: ChannelUpdate<'_>) {
        self.update_channel(context, update);
    }

    fn event(&mut self, context: &mut PluginContext<'_>, event: TelemetryEvent<'_>) {
        match event {
            TelemetryEvent::FrameStart(frame) => {
                if self.callback_trace_enabled() {
                    self.trace_callback(context, &format!(
                        "frame_start pausedSimulationTimeUs={} simulationTimeUs={} renderTimeUs={} timerRestart={}",
                        frame.paused_simulation_time(),
                        frame.simulation_time(),
                        frame.render_time(),
                        frame.timer_restarted()
                    ));
                } else {
                    self.callback_ordinal = self.callback_ordinal.wrapping_add(1);
                }
                if let Some(diagnostic) = self.assembler.frame_start(FrameStart {
                    paused_simulation_time_us: frame.paused_simulation_time(),
                    timer_restart: frame.timer_restarted(),
                }) {
                    Self::bridge_diagnostic(context, diagnostic);
                }
            }
            TelemetryEvent::FrameEnd => {
                self.trace_callback(context, "frame_end");
                match self.assembler.frame_end() {
                    Ok(emitted) => {
                        if self.paused && !emitted.had_channel_callbacks {
                            if matches!(
                                emitted.input,
                                RawInput::Frame(frame) if frame.timer_restart
                            ) {
                                self.process_input(context, RawInput::TimerRestart);
                            }
                            return;
                        }
                        if let Some(diagnostic) = emitted.diagnostic {
                            Self::bridge_diagnostic(context, diagnostic);
                        }
                        self.process_input(context, emitted.input);
                    }
                    Err(diagnostic) => Self::bridge_diagnostic(context, diagnostic),
                }
            }
            TelemetryEvent::Paused => {
                self.trace_callback(context, "paused");
                self.paused = true;
                self.process_input(context, RawInput::Paused);
            }
            TelemetryEvent::Started => {
                self.trace_callback(context, "started");
                self.paused = false;
                self.process_input(context, RawInput::Started);
            }
            TelemetryEvent::Configuration(configuration_event) => {
                let id = configuration_event.id();
                if self.callback_trace_enabled() {
                    self.trace_callback(context, &format!("configuration {id}"));
                } else {
                    self.callback_ordinal = self.callback_ordinal.wrapping_add(1);
                }
                if configuration_event.is(configuration::ids::JOB) {
                    let input = if configuration_event.has_attributes() {
                        RawInput::JobChanged
                    } else {
                        RawInput::JobEnded
                    };
                    self.process_input(context, input);
                }
            }
            TelemetryEvent::Gameplay(gameplay_event) => {
                let id = gameplay_event.id();
                if self.callback_trace_enabled() {
                    self.trace_callback(context, &format!("gameplay {id}"));
                } else {
                    self.callback_ordinal = self.callback_ordinal.wrapping_add(1);
                }
                let input = if gameplay_event.is(gameplay::events::PLAYER_USE_FERRY) {
                    Some(RawInput::FerryUsed)
                } else if gameplay_event.is(gameplay::events::PLAYER_USE_TRAIN) {
                    Some(RawInput::TrainUsed)
                } else if gameplay_event.is(gameplay::events::JOB_CANCELLED)
                    || gameplay_event.is(gameplay::events::JOB_DELIVERED)
                {
                    Some(RawInput::JobEnded)
                } else {
                    None
                };
                if let Some(input) = input {
                    self.process_input(context, input);
                }
            }
        }
    }

    fn shutdown(&mut self, context: &mut PluginContext<'_>) {
        self.trace_callback(context, "source_disconnected");
        self.process_input(context, RawInput::SourceDisconnected);
        self.finish_capture(context);
        self.pipeline = DeterministicPipeline::default();
        context.message(format_args!("[adaptive-eta-live] shutdown"));
    }
}

export_plugin!(TelemetrySpike::default());
