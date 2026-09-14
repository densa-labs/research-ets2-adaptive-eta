use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::raw::RawInput;
use crate::record::{RecordError, VersionedRecord};
use crate::replay::{ReplayReport, ReplayStep, replay};

pub const LIVE_TRACE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTrace {
    pub trace_version: u32,
    pub report: ReplayReport,
}

impl LiveTrace {
    #[must_use]
    pub const fn new(report: ReplayReport) -> Self {
        Self {
            trace_version: LIVE_TRACE_VERSION,
            report,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    Malformed(String),
    UnsupportedVersion(u32),
    Serialization(String),
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) => write!(formatter, "malformed live trace: {message}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported live trace version {version}")
            }
            Self::Serialization(message) => {
                write!(formatter, "could not serialize live trace: {message}")
            }
        }
    }
}

impl Error for TraceError {}

/// Encodes the deterministic live equality surface as JSON.
///
/// # Errors
///
/// Returns an error for an unsupported trace version or JSON serialization
/// failure.
pub fn encode_live_trace(trace: &LiveTrace) -> Result<String, TraceError> {
    if trace.trace_version != LIVE_TRACE_VERSION {
        return Err(TraceError::UnsupportedVersion(trace.trace_version));
    }
    serde_json::to_string_pretty(trace)
        .map_err(|error| TraceError::Serialization(error.to_string()))
}

/// Decodes a deterministic live trace.
///
/// # Errors
///
/// Returns an error for malformed JSON or an unsupported trace version.
pub fn decode_live_trace(input: &str) -> Result<LiveTrace, TraceError> {
    let trace: LiveTrace =
        serde_json::from_str(input).map_err(|error| TraceError::Malformed(error.to_string()))?;
    if trace.trace_version != LIVE_TRACE_VERSION {
        return Err(TraceError::UnsupportedVersion(trace.trace_version));
    }
    Ok(trace)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParityMismatch {
    pub record_index: Option<usize>,
    pub source_epoch: Option<u64>,
    pub sequence: Option<u64>,
    pub surface: &'static str,
    pub live_step: Option<ReplayStep>,
    pub replay_step: Option<ReplayStep>,
    pub live_report: Option<Box<ReplayReport>>,
    pub replay_report: Option<Box<ReplayReport>>,
}

#[derive(Debug)]
pub enum ParityError {
    Replay(RecordError),
    Mismatch(Box<ParityMismatch>),
}

impl fmt::Display for ParityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Replay(error) => error.fmt(formatter),
            Self::Mismatch(mismatch) => write!(
                formatter,
                "parity mismatch surface={} record={:?} sourceEpoch={:?} sequence={:?}\nliveStep={:#?}\nreplayStep={:#?}\nliveReport={:#?}\nreplayReport={:#?}",
                mismatch.surface,
                mismatch.record_index.map(|index| index + 1),
                mismatch.source_epoch,
                mismatch.sequence,
                mismatch.live_step,
                mismatch.replay_step,
                mismatch.live_report,
                mismatch.replay_report,
            ),
        }
    }
}

impl Error for ParityError {}

/// Replays captured inputs and compares the exact typed deterministic report.
///
/// # Errors
///
/// Returns a replay error or a structured mismatch containing the first
/// differing record and its source identity.
pub fn compare_live_trace(
    records: &[VersionedRecord],
    live: &LiveTrace,
) -> Result<(), ParityError> {
    let replayed = replay(records).map_err(ParityError::Replay)?;
    let maximum_steps = live.report.steps.len().max(replayed.steps.len());
    for record_index in 0..maximum_steps {
        let live_step = live.report.steps.get(record_index);
        let replay_step = replayed.steps.get(record_index);
        if live_step != replay_step {
            let (source_epoch, sequence) = records
                .get(record_index)
                .map_or((None, None), |record| input_identity(record.input));
            return Err(ParityError::Mismatch(Box::new(ParityMismatch {
                record_index: Some(record_index),
                source_epoch,
                sequence,
                surface: "step",
                live_step: live_step.cloned(),
                replay_step: replay_step.cloned(),
                live_report: None,
                replay_report: None,
            })));
        }
    }
    if live.report != replayed {
        return Err(ParityError::Mismatch(Box::new(ParityMismatch {
            record_index: None,
            source_epoch: None,
            sequence: None,
            surface: "final-report",
            live_step: None,
            replay_step: None,
            live_report: Some(Box::new(live.report.clone())),
            replay_report: Some(Box::new(replayed)),
        })));
    }
    Ok(())
}

const fn input_identity(input: RawInput) -> (Option<u64>, Option<u64>) {
    match input {
        RawInput::SourceConnected { source_epoch } => (Some(source_epoch), None),
        RawInput::Frame(frame) => (Some(frame.source_epoch), Some(frame.sequence)),
        RawInput::SourceDisconnected
        | RawInput::Paused
        | RawInput::Started
        | RawInput::TimerRestart
        | RawInput::LoadOrRestart
        | RawInput::JobChanged
        | RawInput::JobEnded
        | RawInput::FerryUsed
        | RawInput::TrainUsed => (None, None),
    }
}
