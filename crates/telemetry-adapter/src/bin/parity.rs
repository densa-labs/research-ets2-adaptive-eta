use std::env;
use std::fs;
use std::process::ExitCode;

use telemetry_adapter::{compare_live_trace, decode_jsonl, decode_live_trace};

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let program = arguments.next().unwrap_or_default();
    let Some(capture_path) = arguments.next() else {
        eprintln!(
            "usage: {} CAPTURE.jsonl LIVE_TRACE.json",
            program.to_string_lossy()
        );
        return ExitCode::from(2);
    };
    let Some(trace_path) = arguments.next() else {
        eprintln!("expected a live trace path after the capture path");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("expected exactly two paths");
        return ExitCode::from(2);
    }

    let capture = match fs::read_to_string(&capture_path) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("could not read {}: {error}", capture_path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    let trace = match fs::read_to_string(&trace_path) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("could not read {}: {error}", trace_path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    let records = match decode_jsonl(&capture) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("capture parse failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let live = match decode_live_trace(&trace) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("live trace parse failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    match compare_live_trace(&records, &live) {
        Ok(()) => {
            println!("MATCH records={}", records.len());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("MISMATCH\n{error}");
            ExitCode::FAILURE
        }
    }
}
