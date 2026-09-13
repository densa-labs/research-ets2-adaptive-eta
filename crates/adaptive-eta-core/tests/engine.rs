use adaptive_eta_core::{
    BoundaryReason, CalibrationEngine, EngineConfig, EngineInput, EngineOutput, LifecycleEvent,
    RejectionReason, SampleOutcome, TelemetryFrame,
};

const EPSILON: f64 = 1.0e-10;

#[derive(Clone)]
struct Stream {
    frame: TelemetryFrame,
}

impl Stream {
    fn stabilized(engine: &mut CalibrationEngine) -> Self {
        let mut stream = Self {
            frame: TelemetryFrame {
                sequence: 0,
                source_epoch: 1,
                active_game_time_sec: 0.0,
                active_physical_time_sec: 0.0,
                navigation_distance_m: 10_000_000.0,
                navigation_time_sec: 1_000_000.0,
                odometer_km: 1_000.0,
                speed_mps: 0.0,
                navigation_valid: true,
            },
        };
        engine.process(EngineInput::Event(LifecycleEvent::SourceConnected {
            source_epoch: 1,
        }));
        assert!(engine.process(EngineInput::Frame(stream.frame)).is_none());
        for _ in 0..5 {
            stream.tick(0.0, 0.0, 0.0, 0.0, 0.0);
            let output = engine.process(EngineInput::Frame(stream.frame));
            if stream.frame.active_physical_time_sec < 5.0 {
                assert!(output.is_none());
            } else {
                assert!(matches!(
                    output,
                    Some(EngineOutput::AnchorEstablished { .. })
                ));
            }
        }
        stream
    }

    fn tick(
        &mut self,
        physical_sec: f64,
        game_sec: f64,
        distance_driven_km: f64,
        route_progress_km: f64,
        predicted_consumed_sec: f64,
    ) {
        self.frame.sequence += 1;
        self.frame.active_physical_time_sec += if physical_sec == 0.0 {
            1.0
        } else {
            physical_sec
        };
        self.frame.active_game_time_sec += game_sec;
        self.frame.odometer_km += distance_driven_km;
        self.frame.navigation_distance_m -= route_progress_km * 1_000.0;
        self.frame.navigation_time_sec -= predicted_consumed_sec;
        self.frame.speed_mps = if distance_driven_km > 0.0 { 20.0 } else { 0.0 };
    }

    fn sample(
        &mut self,
        engine: &mut CalibrationEngine,
        ratio: f64,
    ) -> adaptive_eta_core::SampleDecision {
        self.sample_with(engine, ratio, 8.0, 8.0)
    }

    fn sample_with(
        &mut self,
        engine: &mut CalibrationEngine,
        ratio: f64,
        distance_driven_km: f64,
        route_progress_km: f64,
    ) -> adaptive_eta_core::SampleDecision {
        let predicted = 400.0;
        self.tick(
            1.0,
            ratio * predicted,
            distance_driven_km,
            route_progress_km,
            predicted,
        );
        match engine.process(EngineInput::Frame(self.frame)) {
            Some(EngineOutput::Sample(decision)) => decision,
            other => panic!("expected sample decision, got {other:?}"),
        }
    }

    fn stabilize_again(&mut self, engine: &mut CalibrationEngine) {
        for _ in 0..6 {
            self.tick(0.0, 0.0, 0.0, 0.0, 0.0);
            engine.process(EngineInput::Frame(self.frame));
        }
    }
}

fn assert_close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn normal_driver_converges_gradually_below_one() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    let first = stream.sample(&mut engine, 0.90);
    assert!(matches!(
        first.outcome,
        SampleOutcome::AcceptedUnchanged { .. }
    ));
    let after_one = engine.estimator().view();
    assert!((0.90..1.0).contains(&after_one.learned_factor));
    assert!(after_one.display_factor > after_one.learned_factor);

    for _ in 1..250 {
        stream.sample(&mut engine, 0.90);
    }
    let mature = engine.estimator().view();
    assert_close(mature.learned_factor, 0.90, 5.0e-5);
    assert!(mature.display_factor < 1.0);
    assert!(mature.display_factor > mature.learned_factor);
}

#[test]
fn slower_driver_moves_factor_above_one() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    for _ in 0..63 {
        stream.sample(&mut engine, 1.10);
    }
    let view = engine.estimator().view();
    assert!(view.learned_factor > 1.0);
    assert!(view.display_factor > 1.0);
    assert!(view.display_factor < view.learned_factor);
}

#[test]
fn noisy_samples_remain_near_their_log_space_center() {
    let ratios = [0.88, 0.94, 0.91, 0.87, 0.95, 0.90, 0.92, 0.89];
    let geometric_mean = (ratios.iter().copied().map(f64::ln).sum::<f64>() / 8.0).exp();
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    for ratio in ratios.into_iter().cycle().take(240) {
        stream.sample(&mut engine, ratio);
    }
    assert_close(
        engine.estimator().view().learned_factor,
        geometric_mean,
        0.003,
    );
}

#[test]
fn early_confidence_keeps_display_factor_near_one() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.sample(&mut engine, 0.67);
    stream.sample(&mut engine, 0.67);
    let view = engine.estimator().view();
    assert!(view.confidence < 0.08);
    assert!((1.0 - view.display_factor).abs() < (1.0 - view.learned_factor).abs() / 10.0);
}

#[test]
fn mature_confidence_brings_display_factor_toward_learned_factor() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    for _ in 0..250 {
        stream.sample(&mut engine, 0.90);
    }
    let view = engine.estimator().view();
    assert!(view.confidence > 0.90);
    assert!((view.display_factor - view.learned_factor).abs() < 0.01);
}

#[test]
fn short_stationary_stop_remains_inside_sample() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 100.0, 4.0, 4.0, 150.0);
    assert!(engine.process(EngineInput::Frame(stream.frame)).is_none());
    for _ in 0..60 {
        stream.tick(1.0, 1.0, 0.0, 0.0, 0.0);
        assert!(engine.process(EngineInput::Frame(stream.frame)).is_none());
    }
    stream.tick(1.0, 100.0, 4.0, 4.0, 150.0);
    let Some(EngineOutput::Sample(decision)) = engine.process(EngineInput::Frame(stream.frame))
    else {
        panic!("expected sample");
    };
    assert_close(decision.metrics.actual_consumed_sec, 260.0, EPSILON);
    assert_close(decision.metrics.raw_ratio.unwrap(), 260.0 / 300.0, EPSILON);
}

#[test]
fn long_unpaused_stationary_period_discards_window() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    for _ in 0..115 {
        stream.tick(1.0, 3.0, 0.0, 0.0, 0.0);
        let output = engine.process(EngineInput::Frame(stream.frame));
        if let Some(EngineOutput::Boundary(boundary)) = output {
            assert_eq!(boundary.reason, BoundaryReason::LongStationary);
            assert!(boundary.discarded_open_window);
            return;
        }
    }
    panic!("long stationary boundary was not emitted");
}

#[test]
fn pause_discards_and_prevents_cross_boundary_sample() {
    boundary_event_discards(LifecycleEvent::Paused, BoundaryReason::Paused);
}

#[test]
fn timer_restart_is_a_hard_boundary() {
    boundary_event_discards(LifecycleEvent::TimerRestart, BoundaryReason::TimerRestart);
}

#[test]
fn started_job_and_transport_events_are_hard_boundaries() {
    for (event, reason) in [
        (LifecycleEvent::Started, BoundaryReason::Started),
        (LifecycleEvent::JobChanged, BoundaryReason::JobChanged),
        (LifecycleEvent::JobEnded, BoundaryReason::JobEnded),
        (LifecycleEvent::FerryUsed, BoundaryReason::FerryUsed),
        (LifecycleEvent::TrainUsed, BoundaryReason::TrainUsed),
    ] {
        boundary_event_discards(event, reason);
    }
}

#[test]
fn navigation_invalidity_is_a_hard_boundary() {
    boundary_event_discards(
        LifecycleEvent::NavigationBecameInvalid,
        BoundaryReason::NavigationInvalid,
    );
}

#[test]
fn source_disconnect_reconnect_cannot_span_a_sample() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 120.0, 4.0, 4.0, 100.0);
    engine.process(EngineInput::Frame(stream.frame));
    let disconnected = engine.process(EngineInput::Event(LifecycleEvent::SourceDisconnected));
    assert!(matches!(
        disconnected,
        Some(EngineOutput::Boundary(boundary)) if boundary.discarded_open_window
    ));
    engine.process(EngineInput::Event(LifecycleEvent::SourceConnected {
        source_epoch: 2,
    }));
    stream.frame.source_epoch = 2;
    stream.stabilize_again(&mut engine);
    let decision = stream.sample(&mut engine, 0.90);
    assert_close(decision.metrics.distance_driven_km, 8.0, EPSILON);
}

fn boundary_event_discards(event: LifecycleEvent, expected: BoundaryReason) {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 120.0, 4.0, 4.0, 100.0);
    engine.process(EngineInput::Frame(stream.frame));
    let output = engine.process(EngineInput::Event(event));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == expected && boundary.discarded_open_window
    ));
    stream.stabilize_again(&mut engine);
    let decision = stream.sample(&mut engine, 0.90);
    assert_close(decision.metrics.distance_driven_km, 8.0, EPSILON);
}

#[test]
fn reroute_route_length_increase_discards_window() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 1.0;
    stream.frame.active_game_time_sec += 3.0;
    stream.frame.navigation_distance_m += 45_082.0;
    stream.frame.navigation_time_sec += 2_722.0;
    stream.frame.odometer_km += 0.065;
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::NavigationDistanceIncreased
                && boundary.discarded_open_window
    ));
}

#[test]
fn reroute_route_shortening_without_odometer_progress_discards_window() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 3.0, 0.05, 10.0, 500.0);
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::ProgressMismatch
                && boundary.discarded_open_window
    ));
}

#[test]
fn observed_save_load_transient_cannot_create_a_sample() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 1.0;
    stream.frame.navigation_distance_m += 788.126;
    stream.frame.navigation_time_sec += 42.6367;
    let first = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        first,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::NavigationDistanceIncreased
                && boundary.discarded_open_window
    ));

    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 2.0;
    stream.frame.navigation_distance_m -= 788.126;
    stream.frame.navigation_time_sec -= 42.6367;
    let second = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        second,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::ProgressMismatch
                && !boundary.discarded_open_window
    ));
    assert_eq!(engine.estimator().view().valid_sample_count, 0);
}

#[test]
fn explicit_load_boundary_is_supported() {
    boundary_event_discards(LifecycleEvent::LoadOrRestart, BoundaryReason::LoadOrRestart);
}

#[test]
fn source_epoch_change_is_inferred_from_frames() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.source_epoch = 2;
    stream.frame.sequence += 1;
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::SourceEpochChanged
                && boundary.discarded_open_window
    ));
}

#[test]
fn telemetry_gap_discards_window() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 121;
    stream.frame.active_physical_time_sec += 1.0;
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::TelemetryGap
                && boundary.discarded_open_window
    ));
}

#[test]
fn physical_timing_gap_discards_window() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 5.1;
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::TelemetryGap
                && boundary.discarded_open_window
    ));
}

#[test]
fn material_navigation_time_increase_is_a_boundary() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 1.0;
    stream.frame.navigation_time_sec += 31.0;
    let output = engine.process(EngineInput::Frame(stream.frame));
    assert!(matches!(
        output,
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::NavigationTimeIncreased
                && boundary.discarded_open_window
    ));
}

#[test]
fn hard_outlier_does_not_modify_accepted_evidence() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    let before = engine.estimator().view();
    let decision = stream.sample(&mut engine, 2.0);
    assert!(matches!(
        decision.outcome,
        SampleOutcome::Rejected {
            reason: RejectionReason::Outlier
        }
    ));
    assert_eq!(engine.estimator().view(), before);
    assert_eq!(engine.estimator().rejected_outlier(), 1);
}

#[test]
fn plausible_extreme_is_accepted_but_bounded() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    let decision = stream.sample(&mut engine, 1.65);
    assert!(matches!(
        decision.outcome,
        SampleOutcome::AcceptedBounded {
            raw_ratio,
            bounded_ratio
        } if (raw_ratio - 1.65).abs() < EPSILON
            && (bounded_ratio - 1.50).abs() < EPSILON
    ));
    assert_close(decision.update.unwrap().applied_ratio, 1.50, EPSILON);
    assert_eq!(engine.estimator().bounded_sample_count(), 1);
}

#[test]
fn curved_route_and_benign_progress_differences_are_accepted() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    let decision = stream.sample_with(&mut engine, 0.90, 8.0, 9.5);
    assert!(matches!(
        decision.outcome,
        SampleOutcome::AcceptedUnchanged { .. }
    ));
}

#[test]
fn zero_predicted_progress_is_rejected_without_corrupting_state() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 240.0, 8.0, 8.0, 0.0);
    let Some(EngineOutput::Sample(decision)) = engine.process(EngineInput::Frame(stream.frame))
    else {
        panic!("expected rejection");
    };
    assert!(matches!(
        decision.outcome,
        SampleOutcome::Rejected {
            reason: RejectionReason::InsufficientPredictedProgress
        }
    ));
    assert_eq!(engine.estimator().view().valid_sample_count, 0);
}

#[test]
fn zero_elapsed_time_is_rejected_without_corrupting_state() {
    let config = EngineConfig {
        minimum_active_game_time_sec: 0.0,
        ..EngineConfig::default()
    };
    let mut engine = CalibrationEngine::new(config);
    let mut stream = Stream::stabilized(&mut engine);
    stream.tick(1.0, 0.0, 8.0, 8.0, 400.0);
    let Some(EngineOutput::Sample(decision)) = engine.process(EngineInput::Frame(stream.frame))
    else {
        panic!("expected rejection");
    };
    assert!(matches!(
        decision.outcome,
        SampleOutcome::Rejected {
            reason: RejectionReason::NonPositiveActualTime
        }
    ));
    assert_eq!(engine.estimator().view().valid_sample_count, 0);
}

#[test]
fn continued_afk_cannot_reanchor_until_vehicle_moves() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    let mut saw_boundary = false;
    for _ in 0..240 {
        stream.tick(1.0, 3.0, 0.0, 0.0, 0.0);
        match engine.process(EngineInput::Frame(stream.frame)) {
            Some(EngineOutput::Boundary(boundary)) => {
                assert_eq!(boundary.reason, BoundaryReason::LongStationary);
                saw_boundary = true;
            }
            Some(EngineOutput::AnchorEstablished { .. }) => {
                panic!("must not anchor during continued AFK")
            }
            _ => {}
        }
    }
    assert!(saw_boundary);

    stream.tick(1.0, 3.0, 0.01, 0.01, 1.0);
    stream.frame.speed_mps = 1.0;
    assert!(engine.process(EngineInput::Frame(stream.frame)).is_none());
}

#[test]
fn invalid_numbers_and_negative_values_create_boundaries() {
    let mut variants = Vec::new();
    let base = TelemetryFrame {
        sequence: 1,
        source_epoch: 1,
        active_game_time_sec: 1.0,
        active_physical_time_sec: 1.0,
        navigation_distance_m: 1_000.0,
        navigation_time_sec: 100.0,
        odometer_km: 10.0,
        speed_mps: 1.0,
        navigation_valid: true,
    };
    variants.push(TelemetryFrame {
        navigation_time_sec: f64::NAN,
        ..base
    });
    variants.push(TelemetryFrame {
        navigation_distance_m: f64::INFINITY,
        ..base
    });
    variants.push(TelemetryFrame {
        navigation_time_sec: -1.0,
        ..base
    });
    variants.push(TelemetryFrame {
        navigation_distance_m: -1.0,
        ..base
    });

    for frame in variants {
        let mut engine = CalibrationEngine::default();
        let output = engine.process(EngineInput::Frame(frame));
        assert!(matches!(
            output,
            Some(EngineOutput::Boundary(boundary))
                if boundary.reason == BoundaryReason::InvalidTelemetry
        ));
        assert_eq!(engine.estimator().view().valid_sample_count, 0);
    }
}

#[test]
fn odometer_regression_and_clock_regression_are_boundaries() {
    let mut engine = CalibrationEngine::default();
    let mut stream = Stream::stabilized(&mut engine);
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 1.0;
    stream.frame.odometer_km -= 1.0;
    assert!(matches!(
        engine.process(EngineInput::Frame(stream.frame)),
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::OdometerRegressed
    ));

    stream.stabilize_again(&mut engine);
    stream.tick(1.0, 10.0, 0.0, 0.0, 0.0);
    engine.process(EngineInput::Frame(stream.frame));
    stream.frame.sequence += 1;
    stream.frame.active_physical_time_sec += 1.0;
    stream.frame.active_game_time_sec -= 1.0;
    assert!(matches!(
        engine.process(EngineInput::Frame(stream.frame)),
        Some(EngineOutput::Boundary(boundary))
            if boundary.reason == BoundaryReason::TimestampRegressed
    ));
}

#[test]
fn identical_streams_produce_identical_outputs_and_state() {
    fn run() -> (Vec<EngineOutput>, adaptive_eta_core::EstimatorState) {
        let mut engine = CalibrationEngine::default();
        let mut stream = Stream::stabilized(&mut engine);
        let mut outputs = Vec::new();
        for ratio in [0.90, 0.95, 1.10, 0.88] {
            outputs.push(EngineOutput::Sample(stream.sample(&mut engine, ratio)));
        }
        (outputs, engine.estimator().clone())
    }
    assert_eq!(run(), run());
}

#[test]
fn repeated_constant_ratios_move_monotonically_toward_target() {
    for target in [0.90, 1.10] {
        let mut engine = CalibrationEngine::default();
        let mut stream = Stream::stabilized(&mut engine);
        let mut previous = 1.0;
        for _ in 0..100 {
            stream.sample(&mut engine, target);
            let current = engine.estimator().view().learned_factor;
            if target < 1.0 {
                assert!(current < previous && current > target);
            } else {
                assert!(current > previous && current < target);
            }
            assert!(current.is_finite() && current > 0.0);
            let view = engine.estimator().view();
            assert!((0.0..=1.0).contains(&view.confidence));
            assert!(view.display_factor.is_finite() && view.display_factor > 0.0);
            assert!(
                engine
                    .estimator()
                    .adaptive_eta_sec(10_000.0)
                    .unwrap()
                    .is_finite()
            );
            previous = current;
        }
    }
}

#[test]
fn default_config_matches_the_approved_constants() {
    let config = EngineConfig::default();
    assert_close(config.target_distance_km, 8.0, EPSILON);
    assert_close(config.minimum_active_game_time_sec, 240.0, EPSILON);
    assert_close(config.minimum_predicted_consumed_sec, 180.0, EPSILON);
    assert_close(config.maximum_stationary_physical_time_sec, 120.0, EPSILON);
    assert_close(config.hard_ratio_min, 0.50, EPSILON);
    assert_close(config.hard_ratio_max, 1.75, EPSILON);
    assert_close(config.bounded_ratio_min, 0.67, EPSILON);
    assert_close(config.bounded_ratio_max, 1.50, EPSILON);
}

#[test]
fn convergence_checkpoints_are_deterministic() {
    for ratio in [0.90, 1.10] {
        for samples in [13_u64, 63, 250] {
            let mut engine = CalibrationEngine::default();
            let mut stream = Stream::stabilized(&mut engine);
            for _ in 0..samples {
                stream.sample(&mut engine, ratio);
            }
            let view = engine.estimator().view();
            println!(
                "ratio={ratio:.2} distance={:.0} samples={} learned={:.9} confidence={:.9} displayed={:.9}",
                view.valid_observed_distance_km,
                view.valid_sample_count,
                view.learned_factor,
                view.confidence,
                view.display_factor,
            );
            assert!(view.learned_factor.is_finite() && view.learned_factor > 0.0);
            assert!(view.display_factor.is_finite() && view.display_factor > 0.0);
            assert!((0.0..=1.0).contains(&view.confidence));
        }
    }
}
