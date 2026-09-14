use adaptive_eta_core::{
    CalibrationEngine, EngineInput, EngineOutput, EstimatorState, EstimatorView, SampleOutcome,
};
use serde::{Deserialize, Serialize};

use crate::adapter::{AdapterDiagnostic, AdapterOutput, TelemetryAdapter};
use crate::record::{RecordError, VersionedRecord, decode_jsonl};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayStep {
    pub record_index: usize,
    pub adapter_outputs: Vec<AdapterOutput>,
    pub core_outputs: Vec<EngineOutput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub records: usize,
    pub normalized_frames: usize,
    pub adapter_diagnostics: usize,
    pub boundaries: usize,
    pub discarded_windows: usize,
    pub anchors_established: usize,
    pub accepted_samples: usize,
    pub bounded_samples: usize,
    pub rejected_samples: usize,
    pub final_estimator: EstimatorView,
    pub adaptive_eta_sec: Option<f64>,
}

impl Default for ReplaySummary {
    fn default() -> Self {
        Self {
            records: 0,
            normalized_frames: 0,
            adapter_diagnostics: 0,
            boundaries: 0,
            discarded_windows: 0,
            anchors_established: 0,
            accepted_samples: 0,
            bounded_samples: 0,
            rejected_samples: 0,
            final_estimator: CalibrationEngine::default().estimator().view(),
            adaptive_eta_sec: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    pub summary: ReplaySummary,
    pub steps: Vec<ReplayStep>,
    pub adapter_diagnostics: Vec<AdapterDiagnostic>,
    pub final_estimator_state: EstimatorState,
}

/// Stateful deterministic execution path shared by live ingestion and replay.
/// It does not read clocks or files; callers decide whether returned steps are
/// retained as a trace.
#[derive(Clone, Debug, Default)]
pub struct DeterministicPipeline {
    adapter: TelemetryAdapter,
    engine: CalibrationEngine,
    summary: ReplaySummary,
    adapter_diagnostics: Vec<AdapterDiagnostic>,
    latest_game_eta_sec: Option<f64>,
}

impl DeterministicPipeline {
    #[must_use]
    pub fn process(&mut self, record_index: usize, input: crate::raw::RawInput) -> ReplayStep {
        self.summary.records += 1;
        let adapter_outputs = self.adapter.process(input);
        let mut core_outputs = Vec::new();
        for output in &adapter_outputs {
            match output {
                AdapterOutput::Diagnostic(diagnostic) => {
                    self.summary.adapter_diagnostics += 1;
                    self.adapter_diagnostics.push(*diagnostic);
                }
                AdapterOutput::CoreInput(input) => {
                    if let EngineInput::Frame(frame) = input {
                        self.summary.normalized_frames += 1;
                        self.latest_game_eta_sec = Some(frame.navigation_time_sec);
                    }
                    if let Some(core_output) = self.engine.process(*input) {
                        update_summary(&mut self.summary, core_output);
                        core_outputs.push(core_output);
                    }
                }
            }
        }
        ReplayStep {
            record_index,
            adapter_outputs,
            core_outputs,
        }
    }

    #[must_use]
    pub fn report(&self, steps: Vec<ReplayStep>) -> ReplayReport {
        let summary = self.summary();
        ReplayReport {
            summary,
            steps,
            adapter_diagnostics: self.adapter_diagnostics.clone(),
            final_estimator_state: self.engine.estimator().clone(),
        }
    }

    #[must_use]
    pub fn summary(&self) -> ReplaySummary {
        let mut summary = self.summary;
        summary.final_estimator = self.engine.estimator().view();
        summary.adaptive_eta_sec = self
            .latest_game_eta_sec
            .and_then(|game_eta| self.engine.estimator().adaptive_eta_sec(game_eta));
        summary
    }
}

/// Parses and immediately replays a JSONL recording without real-time delays.
///
/// # Errors
///
/// Returns the recording parser's line-numbered schema or version error.
pub fn replay_jsonl(input: &str) -> Result<ReplayReport, RecordError> {
    let records = decode_jsonl(input)?;
    replay(&records)
}

/// Replays already decoded versioned records without real-time delays.
///
/// # Errors
///
/// Returns an error if a caller supplies a record with an unsupported version.
pub fn replay(records: &[VersionedRecord]) -> Result<ReplayReport, RecordError> {
    if let Some((index, record)) = records
        .iter()
        .enumerate()
        .find(|(_, record)| record.record_version != crate::record::RECORD_VERSION)
    {
        return Err(RecordError::UnsupportedVersion {
            line: index + 1,
            found: record.record_version,
        });
    }
    let mut pipeline = DeterministicPipeline::default();
    let mut steps = Vec::with_capacity(records.len());

    for (record_index, record) in records.iter().enumerate() {
        steps.push(pipeline.process(record_index, record.input));
    }
    Ok(pipeline.report(steps))
}

fn update_summary(summary: &mut ReplaySummary, output: EngineOutput) {
    match output {
        EngineOutput::Boundary(boundary) => {
            summary.boundaries += 1;
            if boundary.discarded_open_window {
                summary.discarded_windows += 1;
            }
        }
        EngineOutput::AnchorEstablished { .. } => summary.anchors_established += 1,
        EngineOutput::Sample(decision) => match decision.outcome {
            SampleOutcome::AcceptedUnchanged { .. } => summary.accepted_samples += 1,
            SampleOutcome::AcceptedBounded { .. } => {
                summary.accepted_samples += 1;
                summary.bounded_samples += 1;
            }
            SampleOutcome::Rejected { .. } => summary.rejected_samples += 1,
        },
    }
}
