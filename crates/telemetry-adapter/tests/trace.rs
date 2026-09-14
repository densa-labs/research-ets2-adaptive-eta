use telemetry_adapter::{
    DeterministicPipeline, LiveTrace, ParityError, RawFrame, RawInput, VersionedRecord,
    compare_live_trace, decode_live_trace, encode_jsonl, encode_live_trace, replay,
};

fn records() -> Vec<VersionedRecord> {
    vec![
        VersionedRecord::new(RawInput::SourceConnected { source_epoch: 7 }),
        VersionedRecord::new(RawInput::Frame(RawFrame {
            source_epoch: 7,
            sequence: 1,
            paused_simulation_time_us: 0,
            local_scale: Some(3.0),
            game_time_minutes: Some(100),
            navigation_distance_m: Some(10_000.125),
            navigation_time_sec: Some(700.75),
            speed_mps: Some(12.5),
            odometer_km: Some(2.25),
            timer_restart: false,
        })),
        VersionedRecord::new(RawInput::Paused),
    ]
}

#[test]
fn live_pipeline_trace_exactly_matches_json_round_trip_replay() {
    let records = records();
    let mut pipeline = DeterministicPipeline::default();
    let steps = records
        .iter()
        .enumerate()
        .map(|(index, record)| pipeline.process(index, record.input))
        .collect();
    let live = LiveTrace::new(pipeline.report(steps));

    let capture = encode_jsonl(&records).expect("capture should encode");
    let decoded_records = telemetry_adapter::decode_jsonl(&capture).expect("capture should decode");
    let encoded_trace = encode_live_trace(&live).expect("trace should encode");
    let decoded_trace = decode_live_trace(&encoded_trace).expect("trace should decode");

    assert_eq!(decoded_records, records);
    assert_eq!(decoded_trace, live);
    assert!(compare_live_trace(&decoded_records, &decoded_trace).is_ok());
}

#[test]
fn mismatch_reports_first_record_and_source_identity() {
    let records = records();
    let mut report = replay(&records).expect("records should replay");
    report.steps[1].adapter_outputs.clear();
    let error = compare_live_trace(&records, &LiveTrace::new(report))
        .expect_err("modified trace must not match");

    let ParityError::Mismatch(mismatch) = error else {
        panic!("expected a structured mismatch");
    };
    assert_eq!(mismatch.record_index, Some(1));
    assert_eq!(mismatch.source_epoch, Some(7));
    assert_eq!(mismatch.sequence, Some(1));
    assert_eq!(mismatch.surface, "step");
}
