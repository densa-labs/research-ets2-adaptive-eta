use std::env;
use std::fs;
use std::process::ExitCode;

use adaptive_eta_core::{EngineOutput, SampleOutcome};
use telemetry_adapter::{AdapterOutput, ReplayStep, replay_jsonl};

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let program = arguments.next().unwrap_or_default();
    let Some(path) = arguments.next() else {
        eprintln!("usage: {} RECORDING.jsonl", program.to_string_lossy());
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("expected exactly one recording path");
        return ExitCode::from(2);
    }

    let input = match fs::read_to_string(&path) {
        Ok(input) => input,
        Err(error) => {
            eprintln!("could not read {}: {error}", path.to_string_lossy());
            return ExitCode::FAILURE;
        }
    };
    let report = match replay_jsonl(&input) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("replay failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    for step in &report.steps {
        print_step(step);
    }
    let summary = report.summary;
    println!("records={}", summary.records);
    println!("normalizedFrames={}", summary.normalized_frames);
    println!("adapterDiagnostics={}", summary.adapter_diagnostics);
    println!("boundaries={}", summary.boundaries);
    println!("discardedWindows={}", summary.discarded_windows);
    println!("anchors={}", summary.anchors_established);
    println!("acceptedSamples={}", summary.accepted_samples);
    println!("boundedSamples={}", summary.bounded_samples);
    println!("rejectedSamples={}", summary.rejected_samples);
    println!(
        "learnedFactor={:.9}",
        summary.final_estimator.learned_factor
    );
    println!("confidence={:.9}", summary.final_estimator.confidence);
    println!(
        "displayFactor={:.9}",
        summary.final_estimator.display_factor
    );
    match summary.adaptive_eta_sec {
        Some(value) => println!("adaptiveEtaSec={value:.6}"),
        None => println!("adaptiveEtaSec=null"),
    }
    ExitCode::SUCCESS
}

fn print_step(step: &ReplayStep) {
    let record = step.record_index + 1;
    for output in &step.adapter_outputs {
        if let AdapterOutput::Diagnostic(diagnostic) = output {
            println!(
                "record={record} event=adapterDiagnostic detail={:?}",
                diagnostic.kind
            );
        }
    }
    for output in &step.core_outputs {
        print_core_output(record, output);
    }
}

fn print_core_output(record: usize, output: &EngineOutput) {
    match output {
        EngineOutput::Boundary(boundary) => println!(
            "record={record} event=boundary reason={:?} discardedWindow={}",
            boundary.reason, boundary.discarded_open_window
        ),
        EngineOutput::AnchorEstablished { sequence } => {
            println!("record={record} event=anchor sequence={sequence}");
        }
        EngineOutput::Sample(decision) => {
            let outcome = match decision.outcome {
                SampleOutcome::AcceptedUnchanged { .. } => "accepted",
                SampleOutcome::AcceptedBounded { .. } => "acceptedBounded",
                SampleOutcome::Rejected { .. } => "rejected",
            };
            println!(
                concat!(
                    "record={} event=sample outcome={} ",
                    "actualSec={:.6} predictedSec={:.6} rawRatio={:?}"
                ),
                record,
                outcome,
                decision.metrics.actual_consumed_sec,
                decision.metrics.predicted_consumed_sec,
                decision.metrics.raw_ratio,
            );
            if let Some(update) = decision.update {
                println!(
                    concat!(
                        "record={} event=factorUpdate appliedRatio={:.9} alpha={:.9} ",
                        "oldFactor={:.9} newFactor={:.9}"
                    ),
                    record,
                    update.applied_ratio,
                    update.alpha,
                    update.old_factor,
                    update.new_factor,
                );
            }
        }
    }
}
