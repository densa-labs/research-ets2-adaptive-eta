//! Bounded, local-only production transport for the validated `RawInput` stream.

mod continuity;
#[cfg(unix)]
mod endpoint;
#[cfg(windows)]
#[path = "windows/endpoint.rs"]
mod endpoint;
#[cfg(windows)]
#[path = "windows/handle.rs"]
mod handle;
mod protocol;
mod publish;
#[cfg(unix)]
mod publisher;
#[cfg(windows)]
#[path = "windows/publisher.rs"]
mod publisher;
mod receive;
#[cfg(unix)]
mod receiver;
#[cfg(windows)]
#[path = "windows/receiver.rs"]
mod receiver;

pub use continuity::{ContinuityDiagnostic, ContinuityOutcome, ContinuityTracker};
pub use endpoint::{Endpoint, EndpointError};
pub use protocol::{
    Envelope, MAX_PACKET_SIZE, PROTOCOL_VERSION, ProtocolError, SenderInstance, decode, encode,
};
pub use publish::{PublishCounters, PublishOutcome};
pub use publisher::Publisher;
pub use receive::ReceiveError;
pub use receiver::Receiver;
