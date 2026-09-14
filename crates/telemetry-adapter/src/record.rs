use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::raw::RawInput;

pub const RECORD_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedRecord {
    pub record_version: u32,
    pub input: RawInput,
}

impl VersionedRecord {
    #[must_use]
    pub const fn new(input: RawInput) -> Self {
        Self {
            record_version: RECORD_VERSION,
            input,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    MalformedLine { line: usize, message: String },
    UnsupportedVersion { line: usize, found: u32 },
    Serialization { message: String },
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedLine { line, message } => {
                write!(formatter, "malformed recording line {line}: {message}")
            }
            Self::UnsupportedVersion { line, found } => {
                write!(
                    formatter,
                    "unsupported record version {found} on line {line}"
                )
            }
            Self::Serialization { message } => {
                write!(formatter, "could not serialize recording: {message}")
            }
        }
    }
}

impl Error for RecordError {}

/// Parses a complete JSONL recording.
///
/// # Errors
///
/// Returns a line-numbered error for malformed JSON/schema data or an
/// unsupported per-line record version.
pub fn decode_jsonl(input: &str) -> Result<Vec<VersionedRecord>, RecordError> {
    input
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let line_number = index + 1;
            let record: VersionedRecord =
                serde_json::from_str(line).map_err(|error| RecordError::MalformedLine {
                    line: line_number,
                    message: error.to_string(),
                })?;
            if record.record_version != RECORD_VERSION {
                return Err(RecordError::UnsupportedVersion {
                    line: line_number,
                    found: record.record_version,
                });
            }
            Ok(record)
        })
        .collect()
}

/// Encodes records as one compact JSON object per line.
///
/// # Errors
///
/// Returns an error for an unsupported record version or serialization failure.
pub fn encode_jsonl(records: &[VersionedRecord]) -> Result<String, RecordError> {
    let mut result = String::new();
    for (index, record) in records.iter().enumerate() {
        if record.record_version != RECORD_VERSION {
            return Err(RecordError::UnsupportedVersion {
                line: index + 1,
                found: record.record_version,
            });
        }
        if let RawInput::Frame(frame) = record.input
            && [
                frame.local_scale,
                frame.navigation_distance_m,
                frame.navigation_time_sec,
                frame.speed_mps,
                frame.odometer_km,
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(RecordError::Serialization {
                message: format!("non-finite number on record line {}", index + 1),
            });
        }
        let line = serde_json::to_string(record).map_err(|error| RecordError::Serialization {
            message: error.to_string(),
        })?;
        result.push_str(&line);
        result.push('\n');
    }
    Ok(result)
}
