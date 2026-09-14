use adaptive_eta_core::{EngineInput, LifecycleEvent};
use telemetry_adapter::{
    AdapterDiagnosticKind, AdapterOutput, ClockIssue, RawFrame, RawInput, TelemetryAdapter,
};

fn frame(sequence: u64) -> RawFrame {
    RawFrame {
        source_epoch: 1,
        sequence,
        paused_simulation_time_us: sequence * 1_000_000,
        local_scale: Some(3.0),
        game_time_minutes: Some(100),
        navigation_distance_m: Some(10_000.0),
        navigation_time_sec: Some(600.0),
        speed_mps: Some(10.0),
        odometer_km: Some(100.0),
        timer_restart: false,
    }
}

#[test]
fn valid_frame_becomes_normalized_core_frame() {
    let outputs = TelemetryAdapter::default().process(RawInput::Frame(frame(1)));
    let Some(AdapterOutput::CoreInput(EngineInput::Frame(normalized))) = outputs.last() else {
        panic!("expected normalized frame");
    };
    assert_eq!(normalized.sequence, 1);
    assert_eq!(normalized.source_epoch, 1);
    assert!(normalized.navigation_valid);
    assert!((normalized.navigation_distance_m - 10_000.0).abs() < f64::EPSILON);
    assert!((normalized.active_game_time_sec).abs() < f64::EPSILON);
}

#[test]
fn navigation_unavailability_is_not_coerced_to_zero() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(frame(1)));
    let outputs = adapter.process(RawInput::Frame(RawFrame {
        navigation_distance_m: None,
        navigation_time_sec: None,
        ..frame(2)
    }));
    assert_eq!(outputs.len(), 1);
    assert!(matches!(
        outputs[0],
        AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::NavigationBecameInvalid))
    ));
}

#[test]
fn source_epoch_change_resets_clock_and_emits_diagnostic() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(frame(1)));
    let outputs = adapter.process(RawInput::Frame(RawFrame {
        source_epoch: 2,
        sequence: 1,
        paused_simulation_time_us: 0,
        ..frame(1)
    }));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if matches!(
                diagnostic.kind,
                AdapterDiagnosticKind::SourceEpochChanged {
                    previous: 1,
                    current: 2
                }
            )
    )));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::SourceConnected {
            source_epoch: 2
        }))
    )));
}

#[test]
fn duplicate_regressing_and_gapped_sequences_are_preserved_for_core() {
    for sequence in [7, 6, 500] {
        let outputs = TelemetryAdapter::default().process(RawInput::Frame(RawFrame {
            sequence,
            ..frame(sequence)
        }));
        assert!(outputs.iter().any(|output| matches!(
            output,
            AdapterOutput::CoreInput(EngineInput::Frame(normalized))
                if normalized.sequence == sequence
        )));
    }
}

#[test]
fn lifecycle_events_map_without_calibration_side_effects() {
    for (raw, expected) in [
        (RawInput::Paused, LifecycleEvent::Paused),
        (RawInput::Started, LifecycleEvent::Started),
        (RawInput::TimerRestart, LifecycleEvent::TimerRestart),
        (RawInput::LoadOrRestart, LifecycleEvent::LoadOrRestart),
        (RawInput::JobChanged, LifecycleEvent::JobChanged),
        (RawInput::JobEnded, LifecycleEvent::JobEnded),
        (RawInput::FerryUsed, LifecycleEvent::FerryUsed),
        (RawInput::TrainUsed, LifecycleEvent::TrainUsed),
    ] {
        assert_eq!(
            TelemetryAdapter::default().process(raw),
            vec![AdapterOutput::CoreInput(EngineInput::Event(expected))]
        );
    }
}

#[test]
fn invalid_required_numbers_are_diagnostic_boundaries() {
    let outputs = TelemetryAdapter::default().process(RawInput::Frame(RawFrame {
        odometer_km: Some(f64::NAN),
        speed_mps: Some(f64::INFINITY),
        ..frame(1)
    }));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if matches!(diagnostic.kind, AdapterDiagnosticKind::InvalidOdometer { .. })
    )));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if matches!(diagnostic.kind, AdapterDiagnosticKind::InvalidSpeed { .. })
    )));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::AdapterRejectedInput))
    )));
    assert!(
        !outputs
            .iter()
            .any(|output| matches!(output, AdapterOutput::CoreInput(EngineInput::Frame(_))))
    );
}

#[test]
fn missing_and_invalid_scale_emit_typed_clock_diagnostics() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(RawFrame {
        local_scale: None,
        ..frame(1)
    }));
    let missing = adapter.process(RawInput::Frame(frame(2)));
    assert!(missing.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if diagnostic.kind == AdapterDiagnosticKind::Clock(ClockIssue::MissingIntervalScale)
    )));

    let invalid = adapter.process(RawInput::Frame(RawFrame {
        local_scale: Some(0.0),
        ..frame(3)
    }));
    assert!(invalid.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if matches!(
                diagnostic.kind,
                AdapterDiagnosticKind::Clock(ClockIssue::InvalidScale { value: 0.0 })
            )
    )));
}

#[test]
fn frame_timer_restart_emits_boundary_and_fresh_zero_clock() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(frame(10)));
    let outputs = adapter.process(RawInput::Frame(RawFrame {
        sequence: 11,
        paused_simulation_time_us: 10,
        timer_restart: true,
        ..frame(11)
    }));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::TimerRestart))
    )));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Frame(normalized))
            if normalized.active_game_time_sec.abs() < f64::EPSILON
                && normalized.active_physical_time_sec.abs() < f64::EPSILON
    )));
}

#[test]
fn timestamp_regression_becomes_typed_timing_boundary() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(frame(10)));
    let outputs = adapter.process(RawInput::Frame(RawFrame {
        sequence: 11,
        paused_simulation_time_us: 1,
        ..frame(11)
    }));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::Diagnostic(diagnostic)
            if matches!(
                diagnostic.kind,
                AdapterDiagnosticKind::Clock(ClockIssue::TimestampRegression { .. })
            )
    )));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Event(LifecycleEvent::TimingDiscontinuity))
    )));
}

#[test]
fn paused_timestamp_is_not_counted_by_adapter() {
    let mut adapter = TelemetryAdapter::default();
    let _ = adapter.process(RawInput::Frame(RawFrame {
        paused_simulation_time_us: 1_000_000,
        ..frame(1)
    }));
    let _ = adapter.process(RawInput::Paused);
    let outputs = adapter.process(RawInput::Frame(RawFrame {
        paused_simulation_time_us: 1_000_000,
        ..frame(2)
    }));
    assert!(outputs.iter().any(|output| matches!(
        output,
        AdapterOutput::CoreInput(EngineInput::Frame(normalized))
            if normalized.active_game_time_sec.abs() < f64::EPSILON
                && normalized.active_physical_time_sec.abs() < f64::EPSILON
    )));
}
