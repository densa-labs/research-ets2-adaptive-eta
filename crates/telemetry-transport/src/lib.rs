//! Bounded, local-only production transport for the validated `RawInput` stream.

mod continuity;
#[cfg(unix)]
mod endpoint;
mod protocol;
#[cfg(unix)]
mod publisher;
#[cfg(unix)]
mod receiver;

pub use continuity::{ContinuityDiagnostic, ContinuityOutcome, ContinuityTracker};
#[cfg(unix)]
pub use endpoint::{Endpoint, EndpointError};
pub use protocol::{
    Envelope, MAX_PACKET_SIZE, PROTOCOL_VERSION, ProtocolError, SenderInstance, decode, encode,
};
#[cfg(unix)]
pub use publisher::{PublishCounters, PublishOutcome, Publisher};
#[cfg(unix)]
pub use receiver::{ReceiveError, Receiver};
