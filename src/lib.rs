//! Throwaway macOS feasibility spike for the ETS2 Adaptive ETA project.
//!
//! The plugin deliberately does only three things: subscribe to the minimum
//! telemetry surface, integrate the pause-aware scaled simulation clock, and
//! write rate-limited diagnostic records through the logger supplied by ETS2.

use std::fmt;
use std::time::Duration;

use scs_sdk_plugin::sdk::{ChannelFlags, TelemetryApiVersion, channels, game};
use scs_sdk_plugin::{
    ChannelUpdate, Game, GameCompatibility, PluginCompatibility, PluginContext, PluginMetadata,
    PluginResult, TelemetryEvent, TelemetryEventKind, TelemetryPlugin, export_plugin,
};

// ETS2 caps game.log.txt at roughly 1 MiB. These intentionally verbose records
// exhausted it in about 16 minutes at 2 Hz, so periodic snapshots are limited
// to 0.5 Hz. Scale transitions and lifecycle boundaries are still immediate.
const LOG_INTERVAL_US: u64 = 2_000_000;

static SUPPORTED_GAMES: [GameCompatibility; 1] = [GameCompatibility::new(
    Game::EuroTruckSimulator2,
    game::ets2::V1_12,
)];

#[derive(Clone, Copy, Debug, Default)]
struct ChannelValues {
    local_scale: Option<f32>,
    game_time_minutes: Option<u32>,
    speed_mps: Option<f32>,
    odometer_km: Option<f32>,
    navigation_distance_m: Option<f32>,
    navigation_time_sec: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClockBoundary {
    TimerRestart,
    TimestampWentBackwards,
    ScaleUnavailable,
}

impl fmt::Display for ClockBoundary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimerRestart => formatter.write_str("timerRestart"),
            Self::TimestampWentBackwards => formatter.write_str("timestampWentBackwards"),
            Self::ScaleUnavailable => formatter.write_str("scaleUnavailable"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ClockStep {
    delta_paused_simulation_sec: f64,
    scaled_interval_valid: bool,
    boundary: Option<ClockBoundary>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ScaledClock {
    epoch: u64,
    last_paused_simulation_us: Option<u64>,
    integrated_scaled_active_game_sec: f64,
}

impl ScaledClock {
    fn observe(
        &mut self,
        paused_simulation_us: u64,
        interval_scale: Option<f32>,
        timer_restarted: bool,
    ) -> ClockStep {
        if timer_restarted {
            self.start_new_epoch(paused_simulation_us);
            return ClockStep {
                boundary: Some(ClockBoundary::TimerRestart),
                ..ClockStep::default()
            };
        }

        let Some(previous_us) = self.last_paused_simulation_us else {
            self.last_paused_simulation_us = Some(paused_simulation_us);
            return ClockStep::default();
        };

        let Some(delta_us) = paused_simulation_us.checked_sub(previous_us) else {
            self.start_new_epoch(paused_simulation_us);
            return ClockStep {
                boundary: Some(ClockBoundary::TimestampWentBackwards),
                ..ClockStep::default()
            };
        };
        self.last_paused_simulation_us = Some(paused_simulation_us);

        let delta_sec = Duration::from_micros(delta_us).as_secs_f64();
        if delta_us == 0 {
            return ClockStep {
                delta_paused_simulation_sec: 0.0,
                scaled_interval_valid: interval_scale.is_some_and(valid_scale),
                boundary: None,
            };
        }

        let Some(scale) = interval_scale.filter(|value| valid_scale(*value)) else {
            self.start_new_epoch(paused_simulation_us);
            return ClockStep {
                delta_paused_simulation_sec: delta_sec,
                scaled_interval_valid: false,
                boundary: Some(ClockBoundary::ScaleUnavailable),
            };
        };

        self.integrated_scaled_active_game_sec += delta_sec * f64::from(scale);
        ClockStep {
            delta_paused_simulation_sec: delta_sec,
            scaled_interval_valid: true,
            boundary: None,
        }
    }

    fn start_new_epoch(&mut self, paused_simulation_us: u64) {
        self.epoch = self.epoch.wrapping_add(1);
        self.last_paused_simulation_us = Some(paused_simulation_us);
        self.integrated_scaled_active_game_sec = 0.0;
    }
}

fn valid_scale(scale: f32) -> bool {
    scale.is_finite() && scale > 0.0
}

#[derive(Debug)]
struct TelemetrySpike {
    sequence: u64,
    paused: bool,
    timer_restarted: bool,
    render_time_us: u64,
    simulation_time_us: u64,
    paused_simulation_time_us: u64,
    clock: ScaledClock,
    last_clock_step: ClockStep,
    channels: ChannelValues,
    last_log_render_time_us: Option<u64>,
    last_log_paused_simulation_time_us: Option<u64>,
    last_log_integrated_scaled_sec: Option<f64>,
    last_log_navigation_time_sec: Option<f32>,
    last_log_local_scale: Option<f32>,
}

impl Default for TelemetrySpike {
    fn default() -> Self {
        Self {
            sequence: 0,
            paused: true,
            timer_restarted: false,
            render_time_us: 0,
            simulation_time_us: 0,
            paused_simulation_time_us: 0,
            clock: ScaledClock::default(),
            last_clock_step: ClockStep::default(),
            channels: ChannelValues::default(),
            last_log_render_time_us: None,
            last_log_paused_simulation_time_us: None,
            last_log_integrated_scaled_sec: None,
            last_log_navigation_time_sec: None,
            last_log_local_scale: None,
        }
    }
}

impl TelemetrySpike {
    fn update_channel(&mut self, update: ChannelUpdate<'_>) {
        if update.is(channels::common::LOCAL_SCALE) {
            self.channels.local_scale = update.value(channels::common::LOCAL_SCALE);
        } else if update.is(channels::common::GAME_TIME) {
            self.channels.game_time_minutes = update.value(channels::common::GAME_TIME);
        } else if update.is(channels::truck::SPEED) {
            self.channels.speed_mps = update.value(channels::truck::SPEED);
        } else if update.is(channels::truck::ODOMETER) {
            self.channels.odometer_km = update.value(channels::truck::ODOMETER);
        } else if update.is(channels::truck::NAVIGATION_DISTANCE) {
            self.channels.navigation_distance_m =
                update.value(channels::truck::NAVIGATION_DISTANCE);
        } else if update.is(channels::truck::NAVIGATION_TIME) {
            self.channels.navigation_time_sec = update.value(channels::truck::NAVIGATION_TIME);
        }
    }

    fn observe_frame_start(
        &mut self,
        render_time_us: u64,
        simulation_time_us: u64,
        paused_simulation_time_us: u64,
        timer_restarted: bool,
    ) {
        self.sequence = self.sequence.wrapping_add(1);
        self.render_time_us = render_time_us;
        self.simulation_time_us = simulation_time_us;
        self.paused_simulation_time_us = paused_simulation_time_us;
        self.timer_restarted = timer_restarted;
        self.last_clock_step = self.clock.observe(
            paused_simulation_time_us,
            self.channels.local_scale,
            timer_restarted,
        );

        if self.last_clock_step.boundary.is_some() {
            self.reset_log_anchors();
        }
    }

    fn reset_log_anchors(&mut self) {
        self.last_log_render_time_us = None;
        self.last_log_paused_simulation_time_us = None;
        self.last_log_integrated_scaled_sec = None;
        self.last_log_navigation_time_sec = None;
        self.last_log_local_scale = None;
    }

    fn should_log(&self) -> bool {
        self.last_log_local_scale != self.channels.local_scale
            || self.last_log_render_time_us.is_none_or(|previous| {
                self.render_time_us
                    .checked_sub(previous)
                    .is_none_or(|elapsed| elapsed >= LOG_INTERVAL_US)
            })
    }

    fn log_sample(&mut self, context: &PluginContext<'_>) {
        if !self.should_log() {
            return;
        }

        let delta_paused_since_log = self
            .last_log_paused_simulation_time_us
            .and_then(|previous| self.paused_simulation_time_us.checked_sub(previous))
            .map_or(0.0, |delta| Duration::from_micros(delta).as_secs_f64());
        let delta_scaled_since_log = self.last_log_integrated_scaled_sec.map_or(0.0, |previous| {
            self.clock.integrated_scaled_active_game_sec - previous
        });
        let delta_navigation_since_log = self
            .last_log_navigation_time_sec
            .zip(self.channels.navigation_time_sec)
            .map(|(previous, current)| previous - current);

        context.message(format_args!(
            concat!(
                "[adaptive-eta-spike] sample seq={} clockEpoch={} paused={} timerRestart={} ",
                "pausedSimulationTimeUs={} simulationTimeUs={} renderTimeUs={} ",
                "localScale={} localScaleValid={} gameTimeMinutes={} gameTimeValid={} ",
                "speedMps={} speedValid={} odometerKm={} odometerValid={} ",
                "navigationDistanceM={} navigationTimeSec={} ",
                "navigationDistanceValid={} navigationTimeValid={} ",
                "deltaPausedSimulationSec={:.6} integratedScaledActiveGameSec={:.6} ",
                "deltaIntegratedScaledActiveGameSec={:.6} deltaNavigationTimeSec={} ",
                "lastFrameDeltaPausedSimulationSec={:.6} lastFrameScaleValid={}"
            ),
            self.sequence,
            self.clock.epoch,
            self.paused,
            self.timer_restarted,
            self.paused_simulation_time_us,
            self.simulation_time_us,
            self.render_time_us,
            Optional(self.channels.local_scale),
            self.channels.local_scale.is_some(),
            Optional(self.channels.game_time_minutes),
            self.channels.game_time_minutes.is_some(),
            Optional(self.channels.speed_mps),
            self.channels.speed_mps.is_some(),
            Optional(self.channels.odometer_km),
            self.channels.odometer_km.is_some(),
            Optional(self.channels.navigation_distance_m),
            Optional(self.channels.navigation_time_sec),
            self.channels.navigation_distance_m.is_some(),
            self.channels.navigation_time_sec.is_some(),
            delta_paused_since_log,
            self.clock.integrated_scaled_active_game_sec,
            delta_scaled_since_log,
            Optional(delta_navigation_since_log),
            self.last_clock_step.delta_paused_simulation_sec,
            self.last_clock_step.scaled_interval_valid,
        ));

        self.last_log_render_time_us = Some(self.render_time_us);
        self.last_log_paused_simulation_time_us = Some(self.paused_simulation_time_us);
        self.last_log_integrated_scaled_sec = Some(self.clock.integrated_scaled_active_game_sec);
        self.last_log_navigation_time_sec = self.channels.navigation_time_sec;
        self.last_log_local_scale = self.channels.local_scale;
        self.timer_restarted = false;
    }

    fn log_boundary(&self, context: &PluginContext<'_>, boundary: &str) {
        context.message(format_args!(
            concat!(
                "[adaptive-eta-spike] boundary kind={} seq={} clockEpoch={} paused={} ",
                "pausedSimulationTimeUs={} simulationTimeUs={} renderTimeUs={}"
            ),
            boundary,
            self.sequence,
            self.clock.epoch,
            self.paused,
            self.paused_simulation_time_us,
            self.simulation_time_us,
            self.render_time_us,
        ));
    }
}

struct Optional<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for Optional<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(formatter),
            None => formatter.write_str("null"),
        }
    }
}

impl TelemetryPlugin for TelemetrySpike {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata::new(
            "ETS2 Adaptive ETA Telemetry Spike",
            env!("CARGO_PKG_VERSION"),
        )
    }

    fn compatibility(&self) -> PluginCompatibility {
        PluginCompatibility::new(TelemetryApiVersion::V1_00, &SUPPORTED_GAMES)
    }

    fn initialize(&mut self, context: &mut PluginContext<'_>) -> PluginResult {
        *self = Self::default();

        context.subscribe_event(TelemetryEventKind::FrameStart)?;
        context.subscribe_event(TelemetryEventKind::FrameEnd)?;
        context.subscribe_event(TelemetryEventKind::Paused)?;
        context.subscribe_event(TelemetryEventKind::Started)?;

        let flags = ChannelFlags::EACH_FRAME | ChannelFlags::NO_VALUE;
        context.subscribe_with_flags(channels::common::LOCAL_SCALE, flags)?;
        context.subscribe_with_flags(channels::common::GAME_TIME, flags)?;
        context.subscribe_with_flags(channels::truck::SPEED, flags)?;
        context.subscribe_with_flags(channels::truck::ODOMETER, flags)?;
        context.subscribe_with_flags(channels::truck::NAVIGATION_DISTANCE, flags)?;
        context.subscribe_with_flags(channels::truck::NAVIGATION_TIME, flags)?;

        context.message(format_args!(
            "[adaptive-eta-spike] initialized logIntervalUs={LOG_INTERVAL_US}"
        ));
        Ok(())
    }

    fn channel(&mut self, _context: &mut PluginContext<'_>, update: ChannelUpdate<'_>) {
        self.update_channel(update);
    }

    fn event(&mut self, context: &mut PluginContext<'_>, event: TelemetryEvent<'_>) {
        match event {
            TelemetryEvent::FrameStart(frame) => {
                self.observe_frame_start(
                    frame.render_time(),
                    frame.simulation_time(),
                    frame.paused_simulation_time(),
                    frame.timer_restarted(),
                );
                if let Some(boundary) = self.last_clock_step.boundary {
                    self.log_boundary(context, &boundary.to_string());
                }
            }
            TelemetryEvent::FrameEnd => self.log_sample(context),
            TelemetryEvent::Paused => {
                self.paused = true;
                self.reset_log_anchors();
                self.log_boundary(context, "paused");
            }
            TelemetryEvent::Started => {
                self.paused = false;
                self.reset_log_anchors();
                self.log_boundary(context, "started");
            }
            TelemetryEvent::Configuration(_) | TelemetryEvent::Gameplay(_) => {}
        }
    }

    fn shutdown(&mut self, context: &mut PluginContext<'_>) {
        self.log_boundary(context, "shutdown");
        *self = Self::default();
    }
}

export_plugin!(TelemetrySpike::default());

#[cfg(test)]
mod tests {
    use super::{ClockBoundary, ScaledClock};

    fn assert_near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn integrates_paused_simulation_delta_using_interval_scale() {
        let mut clock = ScaledClock::default();
        clock.observe(1_000_000, None, false);

        let step = clock.observe(2_500_000, Some(19.0), false);

        assert_near(step.delta_paused_simulation_sec, 1.5);
        assert!(step.scaled_interval_valid);
        assert_near(clock.integrated_scaled_active_game_sec, 28.5);
    }

    #[test]
    fn a_static_paused_clock_adds_no_scaled_time() {
        let mut clock = ScaledClock::default();
        clock.observe(5_000_000, Some(19.0), false);

        let step = clock.observe(5_000_000, Some(19.0), false);

        assert_near(step.delta_paused_simulation_sec, 0.0);
        assert_near(clock.integrated_scaled_active_game_sec, 0.0);
    }

    #[test]
    fn scale_changes_apply_per_interval_not_retroactively() {
        let mut clock = ScaledClock::default();
        clock.observe(0, None, false);
        clock.observe(1_000_000, Some(3.0), false);
        clock.observe(2_000_000, Some(19.0), false);

        assert_near(clock.integrated_scaled_active_game_sec, 22.0);
    }

    #[test]
    fn timer_restart_starts_a_new_clock_epoch() {
        let mut clock = ScaledClock::default();
        clock.observe(10_000_000, None, false);
        clock.observe(11_000_000, Some(19.0), false);

        let step = clock.observe(25_000, Some(19.0), true);

        assert_eq!(step.boundary, Some(ClockBoundary::TimerRestart));
        assert_eq!(clock.epoch, 1);
        assert_near(clock.integrated_scaled_active_game_sec, 0.0);
    }

    #[test]
    fn unflagged_backwards_timestamp_is_also_a_hard_boundary() {
        let mut clock = ScaledClock::default();
        clock.observe(10_000_000, None, false);

        let step = clock.observe(1_000_000, Some(19.0), false);

        assert_eq!(step.boundary, Some(ClockBoundary::TimestampWentBackwards));
        assert_eq!(clock.epoch, 1);
        assert_near(clock.integrated_scaled_active_game_sec, 0.0);
    }

    #[test]
    fn missing_scale_during_a_nonzero_interval_is_a_hard_boundary() {
        let mut clock = ScaledClock::default();
        clock.observe(10_000_000, None, false);

        let step = clock.observe(11_000_000, None, false);

        assert_eq!(step.boundary, Some(ClockBoundary::ScaleUnavailable));
        assert_eq!(clock.epoch, 1);
        assert_near(clock.integrated_scaled_active_game_sec, 0.0);
    }
}
