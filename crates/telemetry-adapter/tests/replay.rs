use adaptive_eta_core::{BoundaryReason, EngineInput, EngineOutput};
use telemetry_adapter::{
    AdapterOutput, RawInput, RecordError, ReplayReport, VersionedRecord, replay, replay_jsonl,
};

const NORMAL: &str = include_str!("fixtures/normal_drive.jsonl");
const PAUSE: &str = include_str!("fixtures/pause.jsonl");
const REROUTE: &str = include_str!("fixtures/reroute.jsonl");
const SAVE_LOAD: &str = include_str!("fixtures/save_load_transient.jsonl");
const SCALE: &str = include_str!("fixtures/scale_transition.jsonl");
const NAVIGATION_INVALID: &str = include_str!("fixtures/navigation_invalid.jsonl");
const SOURCE_RESTART: &str = include_str!("fixtures/source_restart.jsonl");

fn replay_fixture(input: &str) -> ReplayReport {
    replay_jsonl(input).expect("fixture must parse and replay")
}

fn boundary_reasons(report: &ReplayReport) -> Vec<BoundaryReason> {
    report
        .steps
        .iter()
        .flat_map(|step| &step.core_outputs)
        .filter_map(|output| match output {
            EngineOutput::Boundary(boundary) => Some(boundary.reason),
            EngineOutput::AnchorEstablished { .. } | EngineOutput::Sample(_) => None,
        })
        .collect()
}

fn final_normalized_clocks(report: &ReplayReport) -> (f64, f64) {
    report
        .steps
        .iter()
        .flat_map(|step| &step.adapter_outputs)
        .filter_map(|output| match output {
            AdapterOutput::CoreInput(EngineInput::Frame(frame)) => {
                Some((frame.active_game_time_sec, frame.active_physical_time_sec))
            }
            AdapterOutput::CoreInput(EngineInput::Event(_)) | AdapterOutput::Diagnostic(_) => None,
        })
        .next_back()
        .expect("fixture must contain a normalized frame")
}

#[test]
fn identical_recording_replays_identically_without_real_time() {
    for fixture in [
        NORMAL,
        PAUSE,
        REROUTE,
        SAVE_LOAD,
        SCALE,
        NAVIGATION_INVALID,
        SOURCE_RESTART,
    ] {
        assert_eq!(replay_fixture(fixture), replay_fixture(fixture));
    }
}

#[test]
fn decoded_records_still_enforce_version() {
    let records = [VersionedRecord {
        record_version: 2,
        input: RawInput::Paused,
    }];
    assert_eq!(
        replay(&records).unwrap_err(),
        RecordError::UnsupportedVersion { line: 1, found: 2 }
    );
}

#[test]
fn normal_drive_accepts_sample_without_false_boundary() {
    let report = replay_fixture(NORMAL);
    assert_eq!(report.summary.records, 4);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(report.summary.rejected_samples, 0);
    assert_eq!(
        boundary_reasons(&report),
        vec![BoundaryReason::SourceConnected]
    );
    assert!(report.summary.final_estimator.learned_factor < 1.0);
}

#[test]
fn pause_discards_partial_window_and_excludes_paused_time() {
    let report = replay_fixture(PAUSE);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(report.summary.anchors_established, 2);
    assert_eq!(report.summary.discarded_windows, 1);
    assert_eq!(
        boundary_reasons(&report),
        vec![
            BoundaryReason::SourceConnected,
            BoundaryReason::Paused,
            BoundaryReason::Started,
        ]
    );
    let (game, physical) = final_normalized_clocks(&report);
    assert!((game - 960.0).abs() < 1.0e-12);
    assert!((physical - 16.0).abs() < 1.0e-12);
}

#[test]
fn reroute_discards_window_then_recovers() {
    let report = replay_fixture(REROUTE);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(report.summary.discarded_windows, 1);
    assert!(boundary_reasons(&report).contains(&BoundaryReason::NavigationDistanceIncreased));
    assert_eq!(report.final_estimator_state.rejected_discontinuity(), 1);
}

#[test]
fn observed_save_load_transient_cannot_cross_into_sample() {
    let report = replay_fixture(SAVE_LOAD);
    let reasons = boundary_reasons(&report);
    assert!(reasons.contains(&BoundaryReason::NavigationDistanceIncreased));
    assert!(reasons.contains(&BoundaryReason::ProgressMismatch));
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(report.summary.rejected_samples, 0);
    assert_eq!(report.summary.discarded_windows, 1);
    assert_eq!(report.final_estimator_state.rejected_discontinuity(), 1);
}

#[test]
fn multiple_scale_transitions_integrate_by_interval_without_boundary() {
    let report = replay_fixture(SCALE);
    let (game, physical) = final_normalized_clocks(&report);
    assert!((game - 267.0).abs() < 1.0e-12);
    assert!((physical - 25.0).abs() < 1.0e-12);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(
        boundary_reasons(&report),
        vec![BoundaryReason::SourceConnected]
    );
}

#[test]
fn navigation_invalidity_is_explicit_and_recovery_stabilizes() {
    let report = replay_fixture(NAVIGATION_INVALID);
    assert_eq!(report.summary.normalized_frames, 6);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(
        boundary_reasons(&report),
        vec![
            BoundaryReason::SourceConnected,
            BoundaryReason::NavigationInvalid,
            BoundaryReason::NavigationBecameValid,
        ]
    );
}

#[test]
fn source_restart_resets_clocks_and_does_not_mix_sessions() {
    let report = replay_fixture(SOURCE_RESTART);
    assert_eq!(report.summary.accepted_samples, 1);
    assert_eq!(report.summary.anchors_established, 2);
    assert_eq!(
        boundary_reasons(&report),
        vec![
            BoundaryReason::SourceConnected,
            BoundaryReason::SourceConnected,
        ]
    );
    assert!((report.summary.final_estimator.valid_observed_distance_km - 8.0).abs() < 1.0e-12);
}
