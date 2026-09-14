#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use adaptive_eta_runtime::RuntimeProcessor;
#[cfg(unix)]
use telemetry_adapter::{LiveTrace, encode_live_trace};
#[cfg(unix)]
use telemetry_transport::{Endpoint, ProtocolError, ReceiveError, Receiver};

#[cfg(unix)]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[cfg(not(unix))]
fn main() {
    eprintln!("Adaptive ETA local transport backend is not implemented for this platform yet");
    std::process::exit(1);
}

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let trace_path = parse_trace_path()?;
    install_signal_handlers();
    let endpoint = Endpoint::for_current_user()?;
    let receiver = Receiver::bind(endpoint)?;
    receiver.set_read_timeout(Some(Duration::from_secs(1)))?;
    println!(
        "waiting for telemetry endpoint={}",
        receiver.endpoint().socket_path().display()
    );

    let mut runtime = RuntimeProcessor::new(trace_path.is_some());
    let mut receiving = false;
    let mut last_status = Instant::now();
    while !SHUTDOWN.load(Ordering::Relaxed) {
        match receiver.receive() {
            Ok(envelope) => {
                receiving = true;
                let result = runtime.ingest(envelope);
                if let Some(diagnostic) = result.diagnostic {
                    eprintln!("transport continuity {diagnostic:?}");
                }
                if last_status.elapsed() >= Duration::from_secs(1) {
                    print_status(runtime.snapshot());
                    last_status = Instant::now();
                }
            }
            Err(ReceiveError::Io(error)) if is_timeout(&error) => {
                if receiving {
                    println!("runtime idle; waiting for telemetry");
                    receiving = false;
                }
            }
            Err(ReceiveError::Protocol(error)) => {
                let unsupported = matches!(error, ProtocolError::UnsupportedVersion { .. });
                runtime.record_malformed(unsupported);
                eprintln!(
                    "{} packet: {error}",
                    if unsupported {
                        "unsupported protocol"
                    } else {
                        "malformed"
                    }
                );
            }
            Err(ReceiveError::Io(error)) => return Err(error.into()),
        }
    }

    if let Some(path) = trace_path {
        let encoded = encode_live_trace(&LiveTrace::new(runtime.report()))?;
        std::fs::write(&path, encoded)?;
        println!("developer transport trace written path={}", path.display());
    }
    println!("runtime shutdown cleanly");
    Ok(())
}

#[cfg(unix)]
fn parse_trace_path() -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(argument) = arguments.next() else {
        return Ok(None);
    };
    if argument != "--trace" {
        return Err(format!("unknown argument: {}", argument.to_string_lossy()).into());
    }
    let path = arguments.next().ok_or("--trace requires a file path")?;
    if arguments.next().is_some() {
        return Err("unexpected extra arguments".into());
    }
    Ok(Some(path.into()))
}

#[cfg(unix)]
fn print_status(snapshot: adaptive_eta_runtime::RuntimeSnapshot) {
    let estimator = snapshot.summary.final_estimator;
    println!(
        "connected source_epoch={} frames={} game_eta={} adaptive_eta={} factor={:.6} confidence={:.4} samples={} transport_missing={}",
        snapshot
            .source_epoch
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        snapshot.summary.normalized_frames,
        format_seconds(snapshot.latest_game_eta_sec),
        format_seconds(snapshot.summary.adaptive_eta_sec),
        estimator.display_factor,
        estimator.confidence,
        estimator.valid_sample_count,
        snapshot.diagnostics.missing_messages,
    );
}

#[cfg(unix)]
fn format_seconds(value: Option<f64>) -> String {
    value.map_or_else(
        || "unavailable".to_owned(),
        |seconds| format!("{seconds:.1}s"),
    )
}

#[cfg(unix)]
fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

#[cfg(unix)]
extern "C" fn request_shutdown(_: i32) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

#[cfg(unix)]
fn install_signal_handlers() {
    unsafe extern "C" {
        fn signal(signal: i32, handler: Option<extern "C" fn(i32)>) -> Option<extern "C" fn(i32)>;
    }
    // SAFETY: the handler only performs a lock-free atomic store, which is
    // async-signal-safe. SIGINT and SIGTERM are the standard Unix values on the
    // supported macOS runtime target.
    unsafe {
        let _ = signal(2, Some(request_shutdown));
        let _ = signal(15, Some(request_shutdown));
    }
}
