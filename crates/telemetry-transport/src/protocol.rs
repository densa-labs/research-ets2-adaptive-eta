use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use telemetry_adapter::{RawFrame, RawInput};

const MAGIC: [u8; 4] = *b"AETA";
const HEADER_SIZE: usize = 32;
const FRAME_PAYLOAD_SIZE: usize = 76;
const FRAME_PACKET_SIZE: usize = HEADER_SIZE + FRAME_PAYLOAD_SIZE;
const SOURCE_CONNECTED_PACKET_SIZE: usize = HEADER_SIZE + 8;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_PACKET_SIZE: usize = 128;

static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SenderInstance(pub [u8; 16]);

impl SenderInstance {
    #[must_use]
    pub fn new_process() -> Self {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let counter = INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let seed = duration.as_secs()
            ^ u64::from(duration.subsec_nanos()).rotate_left(17)
            ^ u64::from(std::process::id()).rotate_left(31)
            ^ counter;
        let first = mix(seed);
        let second = mix(first ^ 0x9e37_79b9_7f4a_7c15);
        let mut bytes = [0_u8; 16];
        bytes[..8].copy_from_slice(&first.to_le_bytes());
        bytes[8..].copy_from_slice(&second.to_le_bytes());
        Self(bytes)
    }
}

impl std::fmt::Display for SenderInstance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Envelope {
    pub sender: SenderInstance,
    pub ordinal: u64,
    pub input: RawInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    PacketTooShort { actual: usize, minimum: usize },
    PacketTooLarge { actual: usize, maximum: usize },
    InvalidMagic,
    UnsupportedVersion { found: u16 },
    InvalidPayloadKind { found: u8 },
    InvalidOrdinal,
    InvalidLength { actual: usize, expected: usize },
    NonzeroReserved,
    NoncanonicalAbsentValue,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ProtocolError {}

/// Encodes one envelope into a caller-owned fixed buffer.
///
/// # Errors
///
/// Returns an error for ordinal zero. All current `RawInput` variants fit in
/// `MAX_PACKET_SIZE` by construction.
pub fn encode(
    envelope: Envelope,
    output: &mut [u8; MAX_PACKET_SIZE],
) -> Result<usize, ProtocolError> {
    if envelope.ordinal == 0 {
        return Err(ProtocolError::InvalidOrdinal);
    }
    output.fill(0);
    output[..4].copy_from_slice(&MAGIC);
    output[4..6].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    output[6] = input_kind(envelope.input);
    output[8..24].copy_from_slice(&envelope.sender.0);
    output[24..32].copy_from_slice(&envelope.ordinal.to_le_bytes());

    match envelope.input {
        RawInput::SourceConnected { source_epoch } => {
            output[32..40].copy_from_slice(&source_epoch.to_le_bytes());
            Ok(SOURCE_CONNECTED_PACKET_SIZE)
        }
        RawInput::Frame(frame) => Ok(encode_frame(frame, &mut output[HEADER_SIZE..])),
        RawInput::SourceDisconnected
        | RawInput::Paused
        | RawInput::Started
        | RawInput::TimerRestart
        | RawInput::TimingDiscontinuity
        | RawInput::LoadOrRestart
        | RawInput::JobChanged
        | RawInput::JobEnded
        | RawInput::FerryUsed
        | RawInput::TrainUsed => Ok(HEADER_SIZE),
    }
}

/// Decodes exactly one bounded datagram.
///
/// # Errors
///
/// Rejects oversized, truncated, malformed, non-canonical, and unsupported
/// packets without reading or allocating from packet-provided lengths.
pub fn decode(packet: &[u8]) -> Result<Envelope, ProtocolError> {
    if packet.len() > MAX_PACKET_SIZE {
        return Err(ProtocolError::PacketTooLarge {
            actual: packet.len(),
            maximum: MAX_PACKET_SIZE,
        });
    }
    if packet.len() < HEADER_SIZE {
        return Err(ProtocolError::PacketTooShort {
            actual: packet.len(),
            minimum: HEADER_SIZE,
        });
    }
    if packet[..4] != MAGIC {
        return Err(ProtocolError::InvalidMagic);
    }
    let version = u16::from_le_bytes([packet[4], packet[5]]);
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion { found: version });
    }
    if packet[7] != 0 {
        return Err(ProtocolError::NonzeroReserved);
    }
    let mut sender = [0_u8; 16];
    sender.copy_from_slice(&packet[8..24]);
    let ordinal = read_u64(&packet[24..32]);
    if ordinal == 0 {
        return Err(ProtocolError::InvalidOrdinal);
    }
    let input = decode_input(packet[6], &packet[HEADER_SIZE..])?;
    Ok(Envelope {
        sender: SenderInstance(sender),
        ordinal,
        input,
    })
}

fn input_kind(input: RawInput) -> u8 {
    match input {
        RawInput::SourceConnected { .. } => 1,
        RawInput::SourceDisconnected => 2,
        RawInput::Frame(_) => 3,
        RawInput::Paused => 4,
        RawInput::Started => 5,
        RawInput::TimerRestart => 6,
        RawInput::TimingDiscontinuity => 7,
        RawInput::LoadOrRestart => 8,
        RawInput::JobChanged => 9,
        RawInput::JobEnded => 10,
        RawInput::FerryUsed => 11,
        RawInput::TrainUsed => 12,
    }
}

fn encode_frame(frame: RawFrame, output: &mut [u8]) -> usize {
    output[0..8].copy_from_slice(&frame.source_epoch.to_le_bytes());
    output[8..16].copy_from_slice(&frame.sequence.to_le_bytes());
    output[16..24].copy_from_slice(&frame.paused_simulation_time_us.to_le_bytes());
    let options = [
        frame.local_scale.is_some(),
        frame.game_time_minutes.is_some(),
        frame.navigation_distance_m.is_some(),
        frame.navigation_time_sec.is_some(),
        frame.speed_mps.is_some(),
        frame.odometer_km.is_some(),
    ];
    output[24] = options
        .iter()
        .enumerate()
        .fold(0_u8, |bits, (index, present)| {
            bits | (u8::from(*present) << index)
        });
    output[25] = u8::from(frame.timer_restart);
    write_f64(&mut output[28..36], frame.local_scale.unwrap_or(0.0));
    output[36..40].copy_from_slice(&frame.game_time_minutes.unwrap_or(0).to_le_bytes());
    write_f64(
        &mut output[44..52],
        frame.navigation_distance_m.unwrap_or(0.0),
    );
    write_f64(
        &mut output[52..60],
        frame.navigation_time_sec.unwrap_or(0.0),
    );
    write_f64(&mut output[60..68], frame.speed_mps.unwrap_or(0.0));
    write_f64(&mut output[68..76], frame.odometer_km.unwrap_or(0.0));
    FRAME_PACKET_SIZE
}

fn decode_input(kind: u8, payload: &[u8]) -> Result<RawInput, ProtocolError> {
    match kind {
        1 => {
            require_length(payload, 8)?;
            Ok(RawInput::SourceConnected {
                source_epoch: read_u64(payload),
            })
        }
        2 => lifecycle(payload, RawInput::SourceDisconnected),
        3 => decode_frame(payload).map(RawInput::Frame),
        4 => lifecycle(payload, RawInput::Paused),
        5 => lifecycle(payload, RawInput::Started),
        6 => lifecycle(payload, RawInput::TimerRestart),
        7 => lifecycle(payload, RawInput::TimingDiscontinuity),
        8 => lifecycle(payload, RawInput::LoadOrRestart),
        9 => lifecycle(payload, RawInput::JobChanged),
        10 => lifecycle(payload, RawInput::JobEnded),
        11 => lifecycle(payload, RawInput::FerryUsed),
        12 => lifecycle(payload, RawInput::TrainUsed),
        found => Err(ProtocolError::InvalidPayloadKind { found }),
    }
}

fn decode_frame(payload: &[u8]) -> Result<RawFrame, ProtocolError> {
    require_length(payload, FRAME_PAYLOAD_SIZE)?;
    if payload[24] & !0x3f != 0
        || payload[25] > 1
        || payload[26..28] != [0, 0]
        || payload[40..44] != [0, 0, 0, 0]
    {
        return Err(ProtocolError::NonzeroReserved);
    }
    let bits = payload[24];
    let local_scale = optional_f64(bits, 0, &payload[28..36])?;
    let game_raw = read_u32(&payload[36..40]);
    let game_time_minutes = optional_u32(bits, 1, game_raw)?;
    Ok(RawFrame {
        source_epoch: read_u64(&payload[0..8]),
        sequence: read_u64(&payload[8..16]),
        paused_simulation_time_us: read_u64(&payload[16..24]),
        local_scale,
        game_time_minutes,
        navigation_distance_m: optional_f64(bits, 2, &payload[44..52])?,
        navigation_time_sec: optional_f64(bits, 3, &payload[52..60])?,
        speed_mps: optional_f64(bits, 4, &payload[60..68])?,
        odometer_km: optional_f64(bits, 5, &payload[68..76])?,
        timer_restart: payload[25] == 1,
    })
}

fn lifecycle(payload: &[u8], input: RawInput) -> Result<RawInput, ProtocolError> {
    require_length(payload, 0)?;
    Ok(input)
}

fn require_length(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(ProtocolError::InvalidLength {
            actual: payload.len(),
            expected,
        })
    }
}

fn optional_f64(bits: u8, bit: u8, bytes: &[u8]) -> Result<Option<f64>, ProtocolError> {
    let value = f64::from_bits(read_u64(bytes));
    optional_value(bits, bit, value, value.to_bits() == 0)
}

fn optional_u32(bits: u8, bit: u8, value: u32) -> Result<Option<u32>, ProtocolError> {
    optional_value(bits, bit, value, value == 0)
}

fn optional_value<T>(
    bits: u8,
    bit: u8,
    value: T,
    absent_is_canonical: bool,
) -> Result<Option<T>, ProtocolError> {
    if bits & (1 << bit) != 0 {
        Ok(Some(value))
    } else if absent_is_canonical {
        Ok(None)
    } else {
        Err(ProtocolError::NoncanonicalAbsentValue)
    }
}

fn write_f64(output: &mut [u8], value: f64) {
    output.copy_from_slice(&value.to_bits().to_le_bytes());
}

fn read_u32(input: &[u8]) -> u32 {
    u32::from_le_bytes(input[..4].try_into().expect("validated fixed-width slice"))
}

fn read_u64(input: &[u8]) -> u64 {
    u64::from_le_bytes(input[..8].try_into().expect("validated fixed-width slice"))
}

const fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender() -> SenderInstance {
        SenderInstance([7; 16])
    }

    fn frame() -> RawFrame {
        RawFrame {
            source_epoch: 9,
            sequence: 42,
            paused_simulation_time_us: 123_456,
            local_scale: Some(19.0),
            game_time_minutes: Some(800),
            navigation_distance_m: None,
            navigation_time_sec: None,
            speed_mps: Some(-2.5),
            odometer_km: Some(55.25),
            timer_restart: true,
        }
    }

    #[test]
    fn every_input_round_trips() {
        let inputs = [
            RawInput::SourceConnected { source_epoch: 4 },
            RawInput::SourceDisconnected,
            RawInput::Frame(frame()),
            RawInput::Paused,
            RawInput::Started,
            RawInput::TimerRestart,
            RawInput::TimingDiscontinuity,
            RawInput::LoadOrRestart,
            RawInput::JobChanged,
            RawInput::JobEnded,
            RawInput::FerryUsed,
            RawInput::TrainUsed,
        ];
        for input in inputs {
            let envelope = Envelope {
                sender: sender(),
                ordinal: 12,
                input,
            };
            let mut buffer = [0; MAX_PACKET_SIZE];
            let size = encode(envelope, &mut buffer).expect("encode");
            assert_eq!(decode(&buffer[..size]), Ok(envelope));
        }
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut buffer = [0; MAX_PACKET_SIZE];
        let size = encode(
            Envelope {
                sender: sender(),
                ordinal: 1,
                input: RawInput::Paused,
            },
            &mut buffer,
        )
        .expect("encode");
        buffer[4..6].copy_from_slice(&2_u16.to_le_bytes());
        assert_eq!(
            decode(&buffer[..size]),
            Err(ProtocolError::UnsupportedVersion { found: 2 })
        );
    }

    #[test]
    fn malformed_truncated_and_oversized_packets_are_rejected() {
        assert!(matches!(
            decode(b"bad"),
            Err(ProtocolError::PacketTooShort { .. })
        ));
        assert_eq!(
            decode(&[0; MAX_PACKET_SIZE + 1]),
            Err(ProtocolError::PacketTooLarge {
                actual: MAX_PACKET_SIZE + 1,
                maximum: MAX_PACKET_SIZE
            })
        );

        let envelope = Envelope {
            sender: sender(),
            ordinal: 1,
            input: RawInput::Frame(frame()),
        };
        let mut buffer = [0; MAX_PACKET_SIZE];
        let size = encode(envelope, &mut buffer).expect("encode");
        assert!(matches!(
            decode(&buffer[..size - 1]),
            Err(ProtocolError::InvalidLength { .. })
        ));
        buffer[7] = 1;
        assert_eq!(decode(&buffer[..size]), Err(ProtocolError::NonzeroReserved));

        let mut lifecycle = [0; MAX_PACKET_SIZE];
        let lifecycle_size = encode(
            Envelope {
                sender: sender(),
                ordinal: 1,
                input: RawInput::Paused,
            },
            &mut lifecycle,
        )
        .expect("encode lifecycle");
        lifecycle[6] = u8::MAX;
        assert_eq!(
            decode(&lifecycle[..lifecycle_size]),
            Err(ProtocolError::InvalidPayloadKind { found: u8::MAX })
        );
    }

    #[test]
    fn maximum_encoded_packet_is_bounded() {
        let mut buffer = [0; MAX_PACKET_SIZE];
        let size = encode(
            Envelope {
                sender: sender(),
                ordinal: 1,
                input: RawInput::Frame(frame()),
            },
            &mut buffer,
        )
        .expect("encode");
        assert_eq!(size, FRAME_PACKET_SIZE);
        assert!(size <= MAX_PACKET_SIZE);
    }
}
