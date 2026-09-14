use std::io;
use std::os::unix::net::UnixDatagram;

use telemetry_adapter::RawInput;

use crate::{
    Endpoint, EndpointError, Envelope, MAX_PACKET_SIZE, PublishCounters, PublishOutcome,
    SenderInstance, encode,
};

#[derive(Debug)]
pub struct Publisher {
    socket: UnixDatagram,
    endpoint: Endpoint,
    sender: SenderInstance,
    next_ordinal: u64,
    buffer: [u8; MAX_PACKET_SIZE],
    counters: PublishCounters,
}

impl Publisher {
    /// Creates an unbound, non-blocking datagram publisher.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket cannot be created or configured.
    pub fn new(endpoint: Endpoint) -> Result<Self, EndpointError> {
        let socket = UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            endpoint,
            sender: SenderInstance::new_process(),
            next_ordinal: 1,
            buffer: [0; MAX_PACKET_SIZE],
            counters: PublishCounters::default(),
        })
    }

    #[must_use]
    pub const fn sender(&self) -> SenderInstance {
        self.sender
    }

    #[must_use]
    pub const fn counters(&self) -> PublishCounters {
        self.counters
    }

    #[must_use]
    pub fn shutdown(self) -> PublishCounters {
        self.counters
    }

    #[must_use]
    pub fn publish(&mut self, input: RawInput) -> PublishOutcome {
        let ordinal = self.next_ordinal;
        self.next_ordinal = self.next_ordinal.checked_add(1).unwrap_or(0);
        self.counters.attempted = self.counters.attempted.saturating_add(1);
        if ordinal == 0 {
            self.counters.serialization_failures =
                self.counters.serialization_failures.saturating_add(1);
            return PublishOutcome::SerializationFailed { ordinal };
        }
        let Ok(size) = encode(
            Envelope {
                sender: self.sender,
                ordinal,
                input,
            },
            &mut self.buffer,
        ) else {
            self.counters.serialization_failures =
                self.counters.serialization_failures.saturating_add(1);
            return PublishOutcome::SerializationFailed { ordinal };
        };
        match self
            .socket
            .send_to(&self.buffer[..size], self.endpoint.socket_path())
        {
            Ok(bytes) => {
                self.counters.successful = self.counters.successful.saturating_add(1);
                self.counters.successful_bytes =
                    self.counters.successful_bytes.saturating_add(bytes as u64);
                self.counters.maximum_packet_bytes = self.counters.maximum_packet_bytes.max(bytes);
                PublishOutcome::Sent { ordinal, bytes }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.counters.would_block_drops = self.counters.would_block_drops.saturating_add(1);
                PublishOutcome::DroppedWouldBlock { ordinal }
            }
            Err(error) if receiver_is_absent(&error) => {
                self.counters.receiver_absent_drops =
                    self.counters.receiver_absent_drops.saturating_add(1);
                PublishOutcome::DroppedReceiverAbsent { ordinal }
            }
            Err(error) => {
                self.counters.other_failures = self.counters.other_failures.saturating_add(1);
                PublishOutcome::OtherFailure {
                    ordinal,
                    kind: error.kind(),
                }
            }
        }
    }
}

fn receiver_is_absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::AddrNotAvailable
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn absent_receiver_never_requires_a_connection_or_retry() {
        let endpoint =
            Endpoint::for_uid_in(123, Path::new("/tmp/does-not-exist-adaptive-eta-test"));
        let mut publisher = Publisher::new(endpoint).expect("publisher");
        assert!(matches!(
            publisher.publish(RawInput::Paused),
            PublishOutcome::DroppedReceiverAbsent { ordinal: 1 }
        ));
        assert_eq!(publisher.counters().attempted, 1);
        assert_eq!(publisher.counters().receiver_absent_drops, 1);
        assert!(matches!(
            publisher.publish(RawInput::Started),
            PublishOutcome::DroppedReceiverAbsent { ordinal: 2 }
        ));
    }
}
