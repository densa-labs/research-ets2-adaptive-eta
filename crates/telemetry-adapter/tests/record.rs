use telemetry_adapter::{
    RECORD_VERSION, RawFrame, RawInput, RecordError, VersionedRecord, decode_jsonl, encode_jsonl,
};

fn unavailable_navigation_record() -> VersionedRecord {
    VersionedRecord::new(RawInput::Frame(RawFrame {
        source_epoch: 7,
        sequence: 42,
        paused_simulation_time_us: 123_456,
        local_scale: Some(19.0),
        game_time_minutes: None,
        navigation_distance_m: None,
        navigation_time_sec: None,
        speed_mps: Some(0.0),
        odometer_km: Some(12.5),
        timer_restart: false,
    }))
}

#[test]
fn jsonl_round_trip_preserves_order_and_unavailable_channels() {
    let records = vec![
        VersionedRecord::new(RawInput::SourceConnected { source_epoch: 7 }),
        unavailable_navigation_record(),
        VersionedRecord::new(RawInput::Paused),
        VersionedRecord::new(RawInput::Started),
    ];
    let encoded = encode_jsonl(&records).unwrap();
    assert!(encoded.contains("\"navigationDistanceM\":null"));
    assert!(encoded.contains("\"navigationTimeSec\":null"));
    assert_eq!(decode_jsonl(&encoded).unwrap(), records);
}

#[test]
fn malformed_line_is_reported_with_line_number() {
    let input = format!(
        "{}not-json\n",
        encode_jsonl(&[VersionedRecord::new(RawInput::Paused)]).unwrap()
    );
    let error = decode_jsonl(&input).unwrap_err();
    assert!(matches!(error, RecordError::MalformedLine { line: 2, .. }));
}

#[test]
fn unsupported_version_is_rejected() {
    let input = r#"{"recordVersion":99,"input":{"type":"paused"}}"#;
    assert_eq!(
        decode_jsonl(input).unwrap_err(),
        RecordError::UnsupportedVersion { line: 1, found: 99 }
    );
}

#[test]
fn missing_required_field_is_malformed() {
    let input = format!(
        r#"{{"recordVersion":{RECORD_VERSION},"input":{{"type":"frame","data":{{"sourceEpoch":1}}}}}}"#
    );
    assert!(matches!(
        decode_jsonl(&input),
        Err(RecordError::MalformedLine { line: 1, .. })
    ));
}

#[test]
fn nonfinite_value_cannot_be_silently_encoded_as_null() {
    let RawInput::Frame(frame) = unavailable_navigation_record().input else {
        unreachable!();
    };
    let record = VersionedRecord::new(RawInput::Frame(RawFrame {
        odometer_km: Some(f64::NAN),
        ..frame
    }));
    assert!(matches!(
        encode_jsonl(&[record]),
        Err(RecordError::Serialization { .. })
    ));
}
