pub mod profile;

use adaptive_eta_core::{
    CalibrationSnapshot, EngineOutput, SampleOutcome, SnapshotValidationError,
};
use telemetry_adapter::{DeterministicPipeline, RawInput, ReplayReport, ReplayStep, ReplaySummary};
use telemetry_transport::{ContinuityDiagnostic, ContinuityTracker, Envelope};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeDiagnostics {
    pub received_packets: u64,
    pub continuity_boundaries: u64,
    pub missing_messages: u64,
    pub duplicates: u64,
    pub out_of_order: u64,
    pub old_sender_packets: u64,
    pub sender_changes: u64,
    pub malformed_packets: u64,
    pub unsupported_protocol_packets: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IngestResult {
    pub diagnostic: Option<ContinuityDiagnostic>,
    pub steps: Vec<ReplayStep>,
    /// Present exactly when this ingest accepted calibration evidence and the
    /// durable estimator state changed.
    pub durable_snapshot: Option<CalibrationSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeSnapshot {
    pub summary: ReplaySummary,
    pub diagnostics: RuntimeDiagnostics,
    pub source_epoch: Option<u64>,
    pub latest_game_eta_sec: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeProcessor {
    continuity: ContinuityTracker,
    pipeline: DeterministicPipeline,
    diagnostics: RuntimeDiagnostics,
    next_record_index: usize,
    trace_steps: Option<Vec<ReplayStep>>,
    source_epoch: Option<u64>,
    latest_game_eta_sec: Option<f64>,
}

impl RuntimeProcessor {
    #[must_use]
    pub fn new(collect_trace: bool) -> Self {
        Self {
            trace_steps: collect_trace.then(Vec::new),
            ..Self::default()
        }
    }

    /// Starts with restored durable estimator evidence while all transport,
    /// adapter, timing, and sampling state remains fresh.
    ///
    /// # Errors
    ///
    /// Returns an error if the snapshot is invalid or unsupported.
    pub fn from_calibration_snapshot(
        collect_trace: bool,
        snapshot: CalibrationSnapshot,
    ) -> Result<Self, SnapshotValidationError> {
        Ok(Self {
            pipeline: DeterministicPipeline::from_calibration_snapshot(snapshot)?,
            trace_steps: collect_trace.then(Vec::new),
            ..Self::default()
        })
    }

    #[must_use]
    pub fn ingest(&mut self, envelope: Envelope) -> IngestResult {
        self.diagnostics.received_packets = self.diagnostics.received_packets.saturating_add(1);
        let outcome = self.continuity.observe(envelope);
        self.record_continuity(outcome.diagnostic);
        let mut steps = Vec::with_capacity(
            usize::from(outcome.requires_boundary) + usize::from(outcome.input.is_some()),
        );
        if outcome.requires_boundary {
            self.diagnostics.continuity_boundaries =
                self.diagnostics.continuity_boundaries.saturating_add(1);
            steps.push(self.process(RawInput::TimingDiscontinuity));
        }
        if let Some(input) = outcome.input {
            self.observe_source(input);
            steps.push(self.process(input));
        }
        let durable_snapshot = steps
            .iter()
            .flat_map(|step| &step.core_outputs)
            .any(is_accepted_sample)
            .then(|| self.pipeline.calibration_snapshot());
        IngestResult {
            diagnostic: outcome.diagnostic,
            steps,
            durable_snapshot,
        }
    }

    pub fn record_malformed(&mut self, unsupported_protocol: bool) {
        if unsupported_protocol {
            self.diagnostics.unsupported_protocol_packets = self
                .diagnostics
                .unsupported_protocol_packets
                .saturating_add(1);
        } else {
            self.diagnostics.malformed_packets =
                self.diagnostics.malformed_packets.saturating_add(1);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            summary: self.pipeline.summary(),
            diagnostics: self.diagnostics,
            source_epoch: self.source_epoch,
            latest_game_eta_sec: self.latest_game_eta_sec,
        }
    }

    #[must_use]
    pub fn report(&self) -> ReplayReport {
        self.pipeline
            .report(self.trace_steps.clone().unwrap_or_default())
    }

    fn process(&mut self, input: RawInput) -> ReplayStep {
        let step = self.pipeline.process(self.next_record_index, input);
        self.next_record_index = self.next_record_index.saturating_add(1);
        if let Some(trace) = &mut self.trace_steps {
            trace.push(step.clone());
        }
        step
    }

    fn observe_source(&mut self, input: RawInput) {
        match input {
            RawInput::SourceConnected { source_epoch } => self.source_epoch = Some(source_epoch),
            RawInput::SourceDisconnected => {
                self.source_epoch = None;
                self.latest_game_eta_sec = None;
            }
            RawInput::Frame(frame) => {
                self.source_epoch = Some(frame.source_epoch);
                self.latest_game_eta_sec = frame.navigation_time_sec;
            }
            RawInput::Paused
            | RawInput::Started
            | RawInput::TimerRestart
            | RawInput::TimingDiscontinuity
            | RawInput::LoadOrRestart
            | RawInput::JobChanged
            | RawInput::JobEnded
            | RawInput::FerryUsed
            | RawInput::TrainUsed => {}
        }
    }

    fn record_continuity(&mut self, diagnostic: Option<ContinuityDiagnostic>) {
        match diagnostic {
            Some(ContinuityDiagnostic::Gap { missing, .. }) => {
                self.diagnostics.missing_messages =
                    self.diagnostics.missing_messages.saturating_add(missing);
            }
            Some(ContinuityDiagnostic::Duplicate { .. }) => {
                self.diagnostics.duplicates = self.diagnostics.duplicates.saturating_add(1);
            }
            Some(ContinuityDiagnostic::OutOfOrder { .. }) => {
                self.diagnostics.out_of_order = self.diagnostics.out_of_order.saturating_add(1);
            }
            Some(ContinuityDiagnostic::OldSender { .. }) => {
                self.diagnostics.old_sender_packets =
                    self.diagnostics.old_sender_packets.saturating_add(1);
            }
            Some(ContinuityDiagnostic::NewSender { .. }) => {
                self.diagnostics.sender_changes = self.diagnostics.sender_changes.saturating_add(1);
            }
            None => {}
        }
    }
}

fn is_accepted_sample(output: &EngineOutput) -> bool {
    matches!(
        output,
        EngineOutput::Sample(decision)
            if matches!(
                decision.outcome,
                SampleOutcome::AcceptedUnchanged { .. } | SampleOutcome::AcceptedBounded { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use adaptive_eta_core::{BoundaryReason, EngineOutput};
    use telemetry_adapter::RawFrame;
    use telemetry_transport::{MAX_PACKET_SIZE, SenderInstance, decode, encode};

    fn envelope(ordinal: u64, input: RawInput) -> Envelope {
        Envelope {
            sender: SenderInstance([1; 16]),
            ordinal,
            input,
        }
    }

    fn frame(sequence: u64, physical_seconds: u32) -> RawInput {
        let seconds = f64::from(physical_seconds);
        RawInput::Frame(RawFrame {
            source_epoch: 1,
            sequence,
            paused_simulation_time_us: u64::from(physical_seconds) * 1_000_000,
            local_scale: Some(3.0),
            game_time_minutes: Some(600),
            navigation_distance_m: Some(20_000.0 - seconds * 100.0),
            navigation_time_sec: Some(2_000.0 - seconds * 5.0),
            speed_mps: Some(20.0),
            odometer_km: Some(100.0 + seconds * 0.1),
            timer_restart: false,
        })
    }

    #[test]
    fn gap_discards_open_window_and_recovers_after_stabilization() {
        let mut runtime = RuntimeProcessor::new(true);
        let _ = runtime.ingest(envelope(1, RawInput::SourceConnected { source_epoch: 1 }));
        for second in 0..=6 {
            let _ = runtime.ingest(envelope(
                u64::from(second + 2),
                frame(u64::from(second + 1), second),
            ));
        }
        let gap = runtime.ingest(envelope(12, frame(11, 10)));
        assert!(matches!(
            gap.diagnostic,
            Some(ContinuityDiagnostic::Gap { missing: 3, .. })
        ));
        assert!(gap.steps.iter().flat_map(|step| &step.core_outputs).any(|output| matches!(output, EngineOutput::Boundary(boundary) if boundary.reason == BoundaryReason::TimingDiscontinuity && boundary.discarded_open_window)));

        for second in 11..=17 {
            let ordinal = u64::from(second + 3);
            let _ = runtime.ingest(envelope(ordinal, frame(u64::from(second + 2), second)));
        }
        assert!(runtime.snapshot().summary.anchors_established >= 2);
        assert_eq!(runtime.snapshot().summary.accepted_samples, 0);
    }

    #[test]
    fn duplicate_is_not_fed_as_new_evidence() {
        let mut runtime = RuntimeProcessor::new(false);
        let _ = runtime.ingest(envelope(1, RawInput::Paused));
        let duplicate = runtime.ingest(envelope(1, RawInput::Paused));
        assert!(duplicate.steps.is_empty());
        assert_eq!(runtime.snapshot().summary.records, 1);
        assert_eq!(runtime.snapshot().diagnostics.duplicates, 1);
    }

    #[test]
    fn sender_change_is_a_boundary_before_new_source_input() {
        let mut runtime = RuntimeProcessor::new(false);
        let _ = runtime.ingest(envelope(1, RawInput::SourceConnected { source_epoch: 1 }));
        let changed = runtime.ingest(Envelope {
            sender: SenderInstance([2; 16]),
            ordinal: 1,
            input: RawInput::SourceConnected { source_epoch: 1 },
        });
        assert_eq!(changed.steps.len(), 2);
        assert!(
            matches!(changed.steps[0].core_outputs.as_slice(), [EngineOutput::Boundary(boundary)] if boundary.reason == BoundaryReason::TimingDiscontinuity)
        );
    }

    #[test]
    fn clean_binary_transport_matches_direct_pipeline_exactly() {
        let mut inputs = vec![RawInput::SourceConnected { source_epoch: 1 }];
        inputs.extend((0..=12).map(|second| frame(u64::from(second + 1), second)));
        inputs.push(RawInput::Paused);
        inputs.push(RawInput::Started);

        let mut direct = DeterministicPipeline::default();
        let direct_steps = inputs
            .iter()
            .copied()
            .enumerate()
            .map(|(index, input)| direct.process(index, input))
            .collect();
        let expected = direct.report(direct_steps);

        let mut transported = RuntimeProcessor::new(true);
        let mut buffer = [0_u8; MAX_PACKET_SIZE];
        for (index, input) in inputs.into_iter().enumerate() {
            let envelope = Envelope {
                sender: SenderInstance([9; 16]),
                ordinal: u64::try_from(index).expect("small index") + 1,
                input,
            };
            let size = encode(envelope, &mut buffer).expect("encode");
            let decoded = decode(&buffer[..size]).expect("decode");
            let _ = transported.ingest(decoded);
        }
        assert_eq!(transported.report(), expected);
    }
}
