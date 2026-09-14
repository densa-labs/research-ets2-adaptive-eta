use telemetry_adapter::{RawFrame, RawInput};

const LOCAL_SCALE: u8 = 1 << 0;
const GAME_TIME: u8 = 1 << 1;
const SPEED: u8 = 1 << 2;
const ODOMETER: u8 = 1 << 3;
const NAVIGATION_DISTANCE: u8 = 1 << 4;
const NAVIGATION_TIME: u8 = 1 << 5;
const ALL_CHANNELS: u8 =
    LOCAL_SCALE | GAME_TIME | SPEED | ODOMETER | NAVIGATION_DISTANCE | NAVIGATION_TIME;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelKind {
    LocalScale,
    GameTime,
    Speed,
    Odometer,
    NavigationDistance,
    NavigationTime,
}

impl ChannelKind {
    const fn mask(self) -> u8 {
        match self {
            Self::LocalScale => LOCAL_SCALE,
            Self::GameTime => GAME_TIME,
            Self::Speed => SPEED,
            Self::Odometer => ODOMETER,
            Self::NavigationDistance => NAVIGATION_DISTANCE,
            Self::NavigationTime => NAVIGATION_TIME,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeDiagnostic {
    FrameStartWhileOpen { abandoned_sequence: u64 },
    FrameEndWithoutStart,
    ChannelOutsideFrame { channel: ChannelKind },
    IncompleteFrame { sequence: u64, missing_mask: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameStart {
    pub paused_simulation_time_us: u64,
    pub timer_restart: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FrameChannels {
    local_scale: Option<f64>,
    game_time_minutes: Option<u32>,
    speed_mps: Option<f64>,
    odometer_km: Option<f64>,
    navigation_distance_m: Option<f64>,
    navigation_time_sec: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OpenFrame {
    sequence: u64,
    start: FrameStart,
    seen: u8,
    channels: FrameChannels,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmittedFrame {
    pub input: RawInput,
    pub diagnostic: Option<BridgeDiagnostic>,
    pub had_channel_callbacks: bool,
}

/// Pure SCS callback-to-`RawInput` frame assembler. Channel values are reset at
/// every frame start, so a missing callback can never silently reuse stale data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveFrameAssembler {
    source_epoch: u64,
    next_sequence: u64,
    open: Option<OpenFrame>,
}

impl LiveFrameAssembler {
    pub fn source_connected(&mut self) -> RawInput {
        self.source_epoch = self.source_epoch.wrapping_add(1).max(1);
        self.next_sequence = 0;
        self.open = None;
        RawInput::SourceConnected {
            source_epoch: self.source_epoch,
        }
    }

    pub fn frame_start(&mut self, start: FrameStart) -> Option<BridgeDiagnostic> {
        let diagnostic = self
            .open
            .map(|frame| BridgeDiagnostic::FrameStartWhileOpen {
                abandoned_sequence: frame.sequence,
            });
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.open = Some(OpenFrame {
            sequence: self.next_sequence,
            start,
            seen: 0,
            channels: FrameChannels::default(),
        });
        diagnostic
    }

    pub fn channel_f64(
        &mut self,
        channel: ChannelKind,
        value: Option<f64>,
    ) -> Option<BridgeDiagnostic> {
        let Some(frame) = &mut self.open else {
            return Some(BridgeDiagnostic::ChannelOutsideFrame { channel });
        };
        frame.seen |= channel.mask();
        match channel {
            ChannelKind::LocalScale => frame.channels.local_scale = value,
            ChannelKind::Speed => frame.channels.speed_mps = value,
            ChannelKind::Odometer => frame.channels.odometer_km = value,
            ChannelKind::NavigationDistance => frame.channels.navigation_distance_m = value,
            ChannelKind::NavigationTime => frame.channels.navigation_time_sec = value,
            ChannelKind::GameTime => {
                return Some(BridgeDiagnostic::ChannelOutsideFrame { channel });
            }
        }
        None
    }

    pub fn channel_game_time(&mut self, value: Option<u32>) -> Option<BridgeDiagnostic> {
        let Some(frame) = &mut self.open else {
            return Some(BridgeDiagnostic::ChannelOutsideFrame {
                channel: ChannelKind::GameTime,
            });
        };
        frame.seen |= GAME_TIME;
        frame.channels.game_time_minutes = value;
        None
    }

    pub fn frame_end(&mut self) -> Result<EmittedFrame, BridgeDiagnostic> {
        let Some(frame) = self.open.take() else {
            return Err(BridgeDiagnostic::FrameEndWithoutStart);
        };
        let missing_mask = ALL_CHANNELS & !frame.seen;
        Ok(EmittedFrame {
            input: RawInput::Frame(RawFrame {
                source_epoch: self.source_epoch,
                sequence: frame.sequence,
                paused_simulation_time_us: frame.start.paused_simulation_time_us,
                local_scale: frame.channels.local_scale,
                game_time_minutes: frame.channels.game_time_minutes,
                navigation_distance_m: frame.channels.navigation_distance_m,
                navigation_time_sec: frame.channels.navigation_time_sec,
                speed_mps: frame.channels.speed_mps,
                odometer_km: frame.channels.odometer_km,
                timer_restart: frame.start.timer_restart,
            }),
            diagnostic: (missing_mask != 0).then_some(BridgeDiagnostic::IncompleteFrame {
                sequence: frame.sequence,
                missing_mask,
            }),
            had_channel_callbacks: frame.seen != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BridgeDiagnostic, ChannelKind, FrameStart, LiveFrameAssembler};
    use adaptive_eta_core::{EngineInput, LifecycleEvent};
    use telemetry_adapter::{AdapterOutput, DeterministicPipeline, RawInput};

    fn complete_frame(assembler: &mut LiveFrameAssembler, scale: Option<f64>) {
        assert_eq!(assembler.channel_f64(ChannelKind::LocalScale, scale), None);
        assert_eq!(assembler.channel_game_time(Some(100)), None);
        assert_eq!(assembler.channel_f64(ChannelKind::Speed, Some(5.0)), None);
        assert_eq!(
            assembler.channel_f64(ChannelKind::Odometer, Some(1.0)),
            None
        );
        assert_eq!(
            assembler.channel_f64(ChannelKind::NavigationDistance, Some(1_000.0)),
            None
        );
        assert_eq!(
            assembler.channel_f64(ChannelKind::NavigationTime, Some(100.0)),
            None
        );
    }

    #[test]
    fn emits_one_complete_snapshot_at_frame_end() {
        let mut assembler = LiveFrameAssembler::default();
        assert_eq!(
            assembler.source_connected(),
            RawInput::SourceConnected { source_epoch: 1 }
        );
        assert_eq!(
            assembler.frame_start(FrameStart {
                paused_simulation_time_us: 50,
                timer_restart: true,
            }),
            None
        );
        complete_frame(&mut assembler, Some(3.0));

        let emitted = assembler.frame_end().expect("frame should be emitted");
        assert_eq!(emitted.diagnostic, None);
        let RawInput::Frame(frame) = emitted.input else {
            panic!("expected a frame");
        };
        assert_eq!(frame.source_epoch, 1);
        assert_eq!(frame.sequence, 1);
        assert_eq!(frame.paused_simulation_time_us, 50);
        assert_eq!(frame.local_scale, Some(3.0));
        assert!(frame.timer_restart);
    }

    #[test]
    fn unavailable_values_do_not_reuse_the_previous_frame() {
        let mut assembler = LiveFrameAssembler::default();
        let _ = assembler.source_connected();
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        complete_frame(&mut assembler, Some(3.0));
        let _ = assembler.frame_end();

        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 1_000_000,
            timer_restart: false,
        });
        complete_frame(&mut assembler, None);
        assert_eq!(
            assembler.channel_f64(ChannelKind::NavigationDistance, None),
            None
        );
        assert_eq!(
            assembler.channel_f64(ChannelKind::NavigationTime, None),
            None
        );
        let emitted = assembler.frame_end().expect("frame should be emitted");
        let RawInput::Frame(frame) = emitted.input else {
            panic!("expected a frame");
        };
        assert_eq!(frame.local_scale, None);
        assert_eq!(frame.navigation_distance_m, None);
        assert_eq!(frame.navigation_time_sec, None);
    }

    #[test]
    fn missing_callback_is_diagnosed_and_preserved_as_unavailable() {
        let mut assembler = LiveFrameAssembler::default();
        let _ = assembler.source_connected();
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        let emitted = assembler.frame_end().expect("frame should be emitted");
        assert!(matches!(
            emitted.diagnostic,
            Some(BridgeDiagnostic::IncompleteFrame { sequence: 1, .. })
        ));
        let RawInput::Frame(frame) = emitted.input else {
            panic!("expected a frame");
        };
        assert_eq!(frame.local_scale, None);
        assert_eq!(frame.speed_mps, None);
    }

    #[test]
    fn source_restart_changes_epoch_and_resets_sequence() {
        let mut assembler = LiveFrameAssembler::default();
        assert_eq!(
            assembler.source_connected(),
            RawInput::SourceConnected { source_epoch: 1 }
        );
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        complete_frame(&mut assembler, Some(3.0));
        let _ = assembler.frame_end();
        assert_eq!(
            assembler.source_connected(),
            RawInput::SourceConnected { source_epoch: 2 }
        );
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: true,
        });
        complete_frame(&mut assembler, Some(3.0));
        let emitted = assembler.frame_end().expect("frame should be emitted");
        let RawInput::Frame(frame) = emitted.input else {
            panic!("expected a frame");
        };
        assert_eq!((frame.source_epoch, frame.sequence), (2, 1));
    }

    #[test]
    fn previous_scale_owns_interval_between_assembled_frames() {
        let mut assembler = LiveFrameAssembler::default();
        let mut pipeline = DeterministicPipeline::default();
        let connected = assembler.source_connected();
        let _ = pipeline.process(0, connected);

        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        complete_frame(&mut assembler, Some(3.0));
        let first = assembler.frame_end().expect("first frame").input;
        let _ = pipeline.process(1, first);

        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 1_000_000,
            timer_restart: false,
        });
        complete_frame(&mut assembler, Some(19.0));
        let second = assembler.frame_end().expect("second frame").input;
        let step = pipeline.process(2, second);
        let normalized =
            step.adapter_outputs
                .iter()
                .find_map(|output| match output {
                    AdapterOutput::CoreInput(EngineInput::Frame(frame)) => Some(frame),
                    AdapterOutput::CoreInput(EngineInput::Event(_))
                    | AdapterOutput::Diagnostic(_) => None,
                })
                .expect("normalized frame");
        assert!((normalized.active_physical_time_sec - 1.0).abs() < f64::EPSILON);
        assert!((normalized.active_game_time_sec - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn lifecycle_input_order_is_preserved_between_frames() {
        let mut pipeline = DeterministicPipeline::default();
        let _ = pipeline.process(0, RawInput::SourceConnected { source_epoch: 1 });
        let paused = pipeline.process(1, RawInput::Paused);
        let started = pipeline.process(2, RawInput::Started);

        assert!(matches!(
            paused.adapter_outputs.as_slice(),
            [AdapterOutput::CoreInput(EngineInput::Event(
                LifecycleEvent::Paused
            ))]
        ));
        assert!(matches!(
            started.adapter_outputs.as_slice(),
            [AdapterOutput::CoreInput(EngineInput::Event(
                LifecycleEvent::Started
            ))]
        ));
        assert_eq!((paused.record_index, started.record_index), (1, 2));
    }

    #[test]
    fn callback_order_anomalies_are_explicit() {
        let mut assembler = LiveFrameAssembler::default();
        let _ = assembler.source_connected();
        assert_eq!(
            assembler.channel_f64(ChannelKind::Speed, Some(1.0)),
            Some(BridgeDiagnostic::ChannelOutsideFrame {
                channel: ChannelKind::Speed
            })
        );
        assert_eq!(
            assembler.frame_end(),
            Err(BridgeDiagnostic::FrameEndWithoutStart)
        );
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        assert_eq!(
            assembler.frame_start(FrameStart {
                paused_simulation_time_us: 1,
                timer_restart: false,
            }),
            Some(BridgeDiagnostic::FrameStartWhileOpen {
                abandoned_sequence: 1
            })
        );
    }

    #[test]
    fn empty_frame_is_distinguishable_from_explicit_no_values() {
        let mut assembler = LiveFrameAssembler::default();
        let _ = assembler.source_connected();
        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: true,
        });
        let empty = assembler.frame_end().expect("empty SDK frame");
        assert!(!empty.had_channel_callbacks);

        let _ = assembler.frame_start(FrameStart {
            paused_simulation_time_us: 0,
            timer_restart: false,
        });
        complete_frame(&mut assembler, None);
        let explicit_no_value = assembler.frame_end().expect("channel callbacks occurred");
        assert!(explicit_no_value.had_channel_callbacks);
    }
}
