use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use telemetry_adapter::{
    LiveTrace, RawInput, ReplayReport, ReplayStep, VersionedRecord, encode_jsonl, encode_live_trace,
};

const CAPTURE_MARKER: &str = "capture.enabled";
const CALLBACK_TRACE_MARKER: &str = "callback-trace.enabled";

#[derive(Debug)]
pub enum CaptureError {
    HomeUnavailable,
    Io(std::io::Error),
    Encode(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("HOME is unavailable"),
            Self::Io(error) => error.fmt(formatter),
            Self::Encode(error) => formatter.write_str(error),
        }
    }
}

impl From<std::io::Error> for CaptureError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug)]
pub struct CapturePaths {
    pub capture: PathBuf,
    pub live_trace: PathBuf,
    pub callback_trace: Option<PathBuf>,
}

#[derive(Debug)]
pub struct DeveloperCapture {
    paths: CapturePaths,
    writer: Option<BufWriter<File>>,
    callback_writer: Option<BufWriter<File>>,
    steps: Vec<ReplayStep>,
    valid: bool,
}

impl DeveloperCapture {
    pub fn open_if_enabled() -> Result<Option<Self>, CaptureError> {
        let developer_root = developer_root()?;
        if !developer_root.join(CAPTURE_MARKER).is_file() {
            return Ok(None);
        }
        let directory = developer_root.join("captures");
        fs::create_dir_all(&directory)?;
        let session = session_id();
        let capture = directory.join(format!("live-{session}.jsonl"));
        let live_trace = directory.join(format!("live-{session}.trace.json"));
        let callback_trace = developer_root
            .join(CALLBACK_TRACE_MARKER)
            .is_file()
            .then(|| directory.join(format!("live-{session}.callbacks.log")));
        let writer = BufWriter::new(File::create(&capture)?);
        let callback_writer = callback_trace
            .as_ref()
            .map(File::create)
            .transpose()?
            .map(BufWriter::new);
        Ok(Some(Self {
            paths: CapturePaths {
                capture,
                live_trace,
                callback_trace,
            },
            writer: Some(writer),
            callback_writer,
            steps: Vec::new(),
            valid: true,
        }))
    }

    pub const fn paths(&self) -> &CapturePaths {
        &self.paths
    }

    pub fn callback_trace_enabled(&self) -> bool {
        self.callback_writer.is_some()
    }

    pub fn record_input(&mut self, input: RawInput) -> Result<(), CaptureError> {
        if !self.valid {
            return Ok(());
        }
        let encoded = match encode_jsonl(&[VersionedRecord::new(input)]) {
            Ok(encoded) => encoded,
            Err(error) => {
                self.valid = false;
                return Err(CaptureError::Encode(error.to_string()));
            }
        };
        let Some(writer) = &mut self.writer else {
            self.valid = false;
            return Err(CaptureError::Encode("capture writer is closed".to_owned()));
        };
        if let Err(error) = writer.write_all(encoded.as_bytes()) {
            self.valid = false;
            return Err(CaptureError::Io(error));
        }
        Ok(())
    }

    pub fn record_step(&mut self, step: ReplayStep) {
        self.steps.push(step);
    }

    pub fn trace_callback(&mut self, ordinal: u64, detail: &str) -> Result<(), CaptureError> {
        let Some(writer) = &mut self.callback_writer else {
            return Ok(());
        };
        if let Err(error) = writeln!(writer, "{ordinal:08} {detail}") {
            self.callback_writer = None;
            return Err(CaptureError::Io(error));
        }
        Ok(())
    }

    pub fn finish(mut self, report: ReplayReport) -> Result<CapturePaths, CaptureError> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
        }
        if let Some(mut writer) = self.callback_writer.take() {
            writer.flush()?;
        }
        if !self.valid {
            return Err(CaptureError::Encode(
                "capture was invalidated by an earlier write failure".to_owned(),
            ));
        }
        let trace = encode_live_trace(&LiveTrace::new(ReplayReport {
            steps: self.steps,
            ..report
        }))
        .map_err(|error| CaptureError::Encode(error.to_string()))?;
        fs::write(&self.paths.live_trace, trace)?;
        Ok(self.paths)
    }
}

fn developer_root() -> Result<PathBuf, CaptureError> {
    let home: OsString = std::env::var_os("HOME").ok_or(CaptureError::HomeUnavailable)?;
    Ok(Path::new(&home).join("Library/Application Support/Adaptive ETA/dev"))
}

fn session_id() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    format!("{seconds}-{}", std::process::id())
}
