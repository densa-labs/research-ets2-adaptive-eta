//! Deterministic bridge from raw-ish SCS telemetry concepts to
//! `adaptive_eta_core` inputs, plus a versioned JSONL record/replay harness.
//!
//! This crate deliberately does not know how live telemetry is transported.
//! Callers supply [`RawInput`] values or versioned recording records.

pub mod adapter;
pub mod clock;
pub mod raw;
pub mod record;
pub mod replay;

pub use adapter::{AdapterDiagnostic, AdapterDiagnosticKind, AdapterOutput, TelemetryAdapter};
pub use clock::{ClockIssue, ClockObservation, IntervalClock};
pub use raw::{RawFrame, RawInput};
pub use record::{RECORD_VERSION, RecordError, VersionedRecord, decode_jsonl, encode_jsonl};
pub use replay::{ReplayReport, ReplayStep, ReplaySummary, replay, replay_jsonl};
