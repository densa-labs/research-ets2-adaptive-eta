use adaptive_eta_core::{EngineInput, LifecycleEvent, TelemetryFrame};
use serde::{Deserialize, Serialize};

use crate::clock::{ClockIssue, IntervalClock};
use crate::raw::{RawFrame, RawInput};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AdapterDiagnosticKind {
    Clock(ClockIssue),
    SourceEpochChanged { previous: u64, current: u64 },
    NavigationAvailabilityMismatch,
    InvalidNavigationDistance { value: f64 },
    InvalidNavigationTime { value: f64 },
    MissingOdometer,
    InvalidOdometer { value: f64 },
    MissingSpeed,
    InvalidSpeed { value: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdapterDiagnostic {
    pub source_epoch: Option<u64>,
    pub sequence: Option<u64>,
    pub kind: AdapterDiagnosticKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AdapterOutput {
    CoreInput(EngineInput),
    Diagnostic(AdapterDiagnostic),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TelemetryAdapter {
    source_epoch: Option<u64>,
    clock: IntervalClock,
    paused: bool,
    navigation_available: Option<bool>,
}

#[derive(Clone, Copy, Debug)]
struct ValidChannels {
    navigation_distance_m: f64,
    navigation_time_sec: f64,
    odometer_km: f64,
    speed_mps: f64,
}

impl TelemetryAdapter {
    #[must_use]
    pub fn process(&mut self, input: RawInput) -> Vec<AdapterOutput> {
        match input {
            RawInput::Frame(frame) => self.process_frame(frame),
            RawInput::SourceConnected { source_epoch } => {
                self.source_epoch = Some(source_epoch);
                self.clock.reset();
                self.navigation_available = None;
                self.paused = false;
                core_event(LifecycleEvent::SourceConnected { source_epoch })
            }
            RawInput::SourceDisconnected => {
                self.clock.reset();
                self.navigation_available = None;
                core_event(LifecycleEvent::SourceDisconnected)
            }
            RawInput::Paused => {
                self.paused = true;
                core_event(LifecycleEvent::Paused)
            }
            RawInput::Started => {
                self.paused = false;
                core_event(LifecycleEvent::Started)
            }
            RawInput::TimerRestart => {
                self.clock.reset();
                self.navigation_available = None;
                core_event(LifecycleEvent::TimerRestart)
            }
            RawInput::TimingDiscontinuity => {
                self.clock.reset();
                self.navigation_available = None;
                core_event(LifecycleEvent::TimingDiscontinuity)
            }
            RawInput::LoadOrRestart => core_event(LifecycleEvent::LoadOrRestart),
            RawInput::JobChanged => core_event(LifecycleEvent::JobChanged),
            RawInput::JobEnded => core_event(LifecycleEvent::JobEnded),
            RawInput::FerryUsed => core_event(LifecycleEvent::FerryUsed),
            RawInput::TrainUsed => core_event(LifecycleEvent::TrainUsed),
        }
    }

    fn process_frame(&mut self, frame: RawFrame) -> Vec<AdapterOutput> {
        let mut outputs = Vec::new();
        if let Some(previous) = self.source_epoch
            && previous != frame.source_epoch
        {
            outputs.push(AdapterOutput::Diagnostic(AdapterDiagnostic {
                source_epoch: Some(frame.source_epoch),
                sequence: Some(frame.sequence),
                kind: AdapterDiagnosticKind::SourceEpochChanged {
                    previous,
                    current: frame.source_epoch,
                },
            }));
            outputs.push(AdapterOutput::CoreInput(EngineInput::Event(
                LifecycleEvent::SourceConnected {
                    source_epoch: frame.source_epoch,
                },
            )));
            self.clock.reset();
            self.navigation_available = None;
        }
        self.source_epoch = Some(frame.source_epoch);

        let clock = self.clock.observe(
            frame.paused_simulation_time_us,
            frame.local_scale,
            self.paused,
            frame.timer_restart,
        );
        if let Some(issue) = clock.issue {
            outputs.push(AdapterOutput::Diagnostic(AdapterDiagnostic {
                source_epoch: Some(frame.source_epoch),
                sequence: Some(frame.sequence),
                kind: AdapterDiagnosticKind::Clock(issue),
            }));
            let event = match issue {
                ClockIssue::TimerRestart => LifecycleEvent::TimerRestart,
                ClockIssue::TimestampRegression { .. }
                | ClockIssue::MissingIntervalScale
                | ClockIssue::InvalidScale { .. }
                | ClockIssue::PausedTimestampAdvanced { .. } => LifecycleEvent::TimingDiscontinuity,
            };
            outputs.push(AdapterOutput::CoreInput(EngineInput::Event(event)));
        }

        let channels = match validate_channels(frame) {
            Ok(None) => {
                self.emit_navigation_invalid_if_needed(&mut outputs);
                return outputs;
            }
            Err(diagnostics) => {
                outputs.extend(
                    diagnostics
                        .into_iter()
                        .map(|kind| frame_diagnostic(frame, kind)),
                );
                self.navigation_available = Some(false);
                outputs.push(adapter_rejected_input());
                return outputs;
            }
            Ok(Some(channels)) => channels,
        };

        if self.navigation_available == Some(false) {
            outputs.push(AdapterOutput::CoreInput(EngineInput::Event(
                LifecycleEvent::NavigationBecameValid,
            )));
        }
        self.navigation_available = Some(true);
        outputs.push(AdapterOutput::CoreInput(EngineInput::Frame(
            TelemetryFrame {
                sequence: frame.sequence,
                source_epoch: frame.source_epoch,
                active_game_time_sec: clock.active_game_time_sec,
                active_physical_time_sec: clock.active_physical_time_sec,
                navigation_distance_m: channels.navigation_distance_m,
                navigation_time_sec: channels.navigation_time_sec,
                odometer_km: channels.odometer_km,
                speed_mps: channels.speed_mps,
                navigation_valid: true,
            },
        )));
        outputs
    }

    fn emit_navigation_invalid_if_needed(&mut self, outputs: &mut Vec<AdapterOutput>) {
        if self.navigation_available != Some(false) {
            outputs.push(AdapterOutput::CoreInput(EngineInput::Event(
                LifecycleEvent::NavigationBecameInvalid,
            )));
        }
        self.navigation_available = Some(false);
    }
}

fn core_event(event: LifecycleEvent) -> Vec<AdapterOutput> {
    vec![AdapterOutput::CoreInput(EngineInput::Event(event))]
}

fn adapter_rejected_input() -> AdapterOutput {
    AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::AdapterRejectedInput))
}

fn frame_diagnostic(frame: RawFrame, kind: AdapterDiagnosticKind) -> AdapterOutput {
    AdapterOutput::Diagnostic(AdapterDiagnostic {
        source_epoch: Some(frame.source_epoch),
        sequence: Some(frame.sequence),
        kind,
    })
}

fn validate_channels(frame: RawFrame) -> Result<Option<ValidChannels>, Vec<AdapterDiagnosticKind>> {
    let (navigation_distance_m, navigation_time_sec) =
        match (frame.navigation_distance_m, frame.navigation_time_sec) {
            (None, None) => return Ok(None),
            (Some(_), None) | (None, Some(_)) => {
                return Err(vec![AdapterDiagnosticKind::NavigationAvailabilityMismatch]);
            }
            (Some(distance), Some(time)) => (distance, time),
        };

    let mut diagnostics = Vec::new();
    if !is_nonnegative_finite(navigation_distance_m) {
        diagnostics.push(AdapterDiagnosticKind::InvalidNavigationDistance {
            value: navigation_distance_m,
        });
    }
    if !is_nonnegative_finite(navigation_time_sec) {
        diagnostics.push(AdapterDiagnosticKind::InvalidNavigationTime {
            value: navigation_time_sec,
        });
    }
    let odometer_km = validate_odometer(frame.odometer_km, &mut diagnostics);
    let speed_mps = validate_speed(frame.speed_mps, &mut diagnostics);

    if diagnostics.is_empty() {
        Ok(Some(ValidChannels {
            navigation_distance_m,
            navigation_time_sec,
            odometer_km,
            speed_mps,
        }))
    } else {
        Err(diagnostics)
    }
}

fn validate_odometer(value: Option<f64>, diagnostics: &mut Vec<AdapterDiagnosticKind>) -> f64 {
    match value {
        None => {
            diagnostics.push(AdapterDiagnosticKind::MissingOdometer);
            0.0
        }
        Some(value) if !is_nonnegative_finite(value) => {
            diagnostics.push(AdapterDiagnosticKind::InvalidOdometer { value });
            0.0
        }
        Some(value) => value,
    }
}

fn validate_speed(value: Option<f64>, diagnostics: &mut Vec<AdapterDiagnosticKind>) -> f64 {
    match value {
        None => {
            diagnostics.push(AdapterDiagnosticKind::MissingSpeed);
            0.0
        }
        Some(value) if !value.is_finite() => {
            diagnostics.push(AdapterDiagnosticKind::InvalidSpeed { value });
            0.0
        }
        Some(value) => value,
    }
}

fn is_nonnegative_finite(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}
