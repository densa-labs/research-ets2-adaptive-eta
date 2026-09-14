use adaptive_eta_core::EngineOutput;
use adaptive_eta_runtime::RuntimeProcessor;
use adaptive_eta_runtime::profile::{ProfileState, ProfileStore};
use telemetry_adapter::{RawFrame, RawInput};
use telemetry_transport::{Envelope, SenderInstance};

fn envelope(sender: u8, ordinal: u64, input: RawInput) -> Envelope {
    Envelope {
        sender: SenderInstance([sender; 16]),
        ordinal,
        input,
    }
}

fn frame(epoch: u64, sequence: u64, second: u32, odometer_base: f64) -> RawInput {
    let second_f64 = f64::from(second);
    RawInput::Frame(RawFrame {
        source_epoch: epoch,
        sequence,
        paused_simulation_time_us: u64::from(second) * 1_000_000,
        local_scale: Some(3.0),
        game_time_minutes: Some(600),
        navigation_distance_m: Some(50_000.0 - second_f64 * 125.0),
        navigation_time_sec: Some(5_000.0 - second_f64 * 2.5),
        speed_mps: Some(20.0),
        odometer_km: Some(odometer_base + second_f64 * 0.125),
        timer_restart: false,
    })
}

fn start_session(runtime: &mut RuntimeProcessor, sender: u8, epoch: u64) -> u64 {
    let _ = runtime.ingest(envelope(
        sender,
        1,
        RawInput::SourceConnected {
            source_epoch: epoch,
        },
    ));
    2
}

fn feed_range(
    runtime: &mut RuntimeProcessor,
    sender: u8,
    epoch: u64,
    ordinal_start: u64,
    seconds: std::ops::RangeInclusive<u32>,
    odometer_base: f64,
) -> (u64, Option<adaptive_eta_core::CalibrationSnapshot>) {
    let mut ordinal = ordinal_start;
    let mut durable = None;
    for second in seconds {
        let result = runtime.ingest(envelope(
            sender,
            ordinal,
            frame(epoch, u64::from(second) + 1, second, odometer_base),
        ));
        if result.durable_snapshot.is_some() {
            durable = result.durable_snapshot;
        }
        ordinal += 1;
    }
    (ordinal, durable)
}

#[test]
fn first_run_is_neutral() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path().join("profiles/ets2/default.json"));
    assert_eq!(store.load().state, ProfileState::Fresh);

    let mut runtime = RuntimeProcessor::new(false);
    let ordinal = start_session(&mut runtime, 1, 1);
    let _ = feed_range(&mut runtime, 1, 1, ordinal, 0..=0, 100.0);
    let snapshot = runtime.snapshot();
    assert_eq!(snapshot.summary.final_estimator.valid_sample_count, 0);
    assert_eq!(
        snapshot.summary.adaptive_eta_sec,
        snapshot.latest_game_eta_sec
    );
}

#[test]
fn restart_restores_learning_but_not_transient_state_and_continues_exactly() {
    let directory = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(directory.path().join("profiles/ets2/default.json"));

    let mut uninterrupted = RuntimeProcessor::new(true);
    let ordinal = start_session(&mut uninterrupted, 1, 1);
    let (ordinal, durable) = feed_range(&mut uninterrupted, 1, 1, ordinal, 0..=85, 100.0);
    let durable = durable.expect("first sample accepted");
    assert_eq!(durable.sample_count, 1);
    store.save(durable).unwrap();
    let eta_before_restart = uninterrupted.snapshot().summary.adaptive_eta_sec;
    let game_eta_before_restart = uninterrupted.snapshot().latest_game_eta_sec.unwrap();

    // Leave a live sample window open. It must not be represented in the saved profile.
    let (uninterrupted_ordinal, unexpected_save) =
        feed_range(&mut uninterrupted, 1, 1, ordinal, 86..=100, 100.0);
    assert!(unexpected_save.is_none());

    let loaded = match store.load().state {
        ProfileState::Loaded(snapshot) => snapshot,
        state => panic!("expected loaded profile, got {state:?}"),
    };
    assert_eq!(loaded, durable);
    let mut restarted = RuntimeProcessor::from_calibration_snapshot(true, loaded).unwrap();
    assert_eq!(
        restarted.snapshot().summary.final_estimator,
        adaptive_eta_core::EstimatorState::from_snapshot(loaded)
            .unwrap()
            .view()
    );
    assert_eq!(
        adaptive_eta_core::EstimatorState::from_snapshot(loaded)
            .unwrap()
            .adaptive_eta_sec(game_eta_before_restart),
        eta_before_restart
    );

    let ordinal = start_session(&mut restarted, 2, 2);
    for second in 0..5 {
        let result = restarted.ingest(envelope(
            2,
            ordinal + u64::from(second),
            frame(2, u64::from(second) + 1, second, 500.0),
        ));
        assert!(
            !result
                .steps
                .iter()
                .flat_map(|step| &step.core_outputs)
                .any(|output| matches!(output, EngineOutput::AnchorEstablished { .. }))
        );
        assert!(result.durable_snapshot.is_none());
    }
    let anchor = restarted.ingest(envelope(2, ordinal + 5, frame(2, 6, 5, 500.0)));
    assert!(
        anchor
            .steps
            .iter()
            .flat_map(|step| &step.core_outputs)
            .any(|output| matches!(output, EngineOutput::AnchorEstablished { .. }))
    );

    let (_, restarted_second) = feed_range(&mut restarted, 2, 2, ordinal + 6, 6..=85, 500.0);
    let restarted_second = restarted_second.expect("second sample accepted after restart");

    let (_, uninterrupted_second) = feed_range(
        &mut uninterrupted,
        1,
        1,
        uninterrupted_ordinal,
        101..=165,
        100.0,
    );
    let uninterrupted_second = uninterrupted_second.expect("uninterrupted second sample accepted");
    assert_eq!(restarted_second, uninterrupted_second);
    assert_eq!(restarted_second.sample_count, 2);
}
