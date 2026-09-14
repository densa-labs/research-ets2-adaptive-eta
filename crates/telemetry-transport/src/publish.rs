use std::io;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishCounters {
    pub attempted: u64,
    pub successful: u64,
    pub successful_bytes: u64,
    pub maximum_packet_bytes: usize,
    pub would_block_drops: u64,
    pub receiver_absent_drops: u64,
    pub serialization_failures: u64,
    pub other_failures: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    Sent { ordinal: u64, bytes: usize },
    Pending { ordinal: u64, bytes: usize },
    DroppedWouldBlock { ordinal: u64 },
    DroppedReceiverAbsent { ordinal: u64 },
    SerializationFailed { ordinal: u64 },
    OtherFailure { ordinal: u64, kind: io::ErrorKind },
}
