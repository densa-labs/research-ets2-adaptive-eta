use std::io;
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

use crate::{Endpoint, EndpointError, Envelope, MAX_PACKET_SIZE, ProtocolError, decode};

#[derive(Debug)]
pub enum ReceiveError {
    Io(io::Error),
    Protocol(ProtocolError),
}

impl std::fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Protocol(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReceiveError {}

#[derive(Debug)]
pub struct Receiver {
    socket: UnixDatagram,
    endpoint: Endpoint,
    socket_identity: (u64, u64),
}

impl Receiver {
    /// Securely recreates and binds the per-user endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe pre-existing paths, another active receiver,
    /// or an operating-system socket failure.
    pub fn bind(endpoint: Endpoint) -> Result<Self, EndpointError> {
        endpoint.prepare_for_bind()?;
        let socket = UnixDatagram::bind(endpoint.socket_path())?;
        if let Err(error) = endpoint.secure_bound_socket() {
            endpoint.cleanup_owned_socket(None);
            return Err(error);
        }
        let socket_identity = match endpoint.socket_identity() {
            Ok(identity) => identity,
            Err(error) => {
                endpoint.cleanup_owned_socket(None);
                return Err(error);
            }
        };
        Ok(Self {
            socket,
            endpoint,
            socket_identity,
        })
    }

    /// Sets a timeout so callers can report idle state and observe shutdown.
    ///
    /// # Errors
    ///
    /// Returns an operating-system error if the timeout cannot be set.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_read_timeout(timeout)
    }

    /// Receives and decodes one complete datagram.
    ///
    /// # Errors
    ///
    /// Returns a typed I/O or bounded protocol error. A datagram larger than the
    /// protocol limit is read into one extra byte and rejected as oversized.
    pub fn receive(&self) -> Result<Envelope, ReceiveError> {
        let mut buffer = [0_u8; MAX_PACKET_SIZE + 1];
        let size = self.socket.recv(&mut buffer).map_err(ReceiveError::Io)?;
        decode(&buffer[..size]).map_err(ReceiveError::Protocol)
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.endpoint
            .cleanup_owned_socket(Some(self.socket_identity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContinuityDiagnostic, ContinuityTracker, EndpointError, PublishOutcome, Publisher,
    };
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    use telemetry_adapter::RawInput;

    static TEMP_ORDINAL: AtomicU64 = AtomicU64::new(1);

    fn isolated_endpoint() -> (std::path::PathBuf, Endpoint) {
        let ordinal = TEMP_ORDINAL.fetch_add(1, Ordering::Relaxed);
        let base = std::path::Path::new("/private/tmp")
            .join(format!("aet-test-{}-{ordinal}", std::process::id()));
        fs::create_dir(&base).expect("create test base");
        let uid = fs::metadata(&base).expect("test metadata").uid();
        let endpoint = Endpoint::for_uid_in(uid, &base);
        (base, endpoint)
    }

    fn cleanup(base: &std::path::Path) {
        let _ = fs::remove_dir_all(base);
    }

    fn bind_or_skip(endpoint: Endpoint) -> Option<Receiver> {
        match Receiver::bind(endpoint) {
            Ok(receiver) => Some(receiver),
            Err(EndpointError::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "Unix socket creation is denied by this test sandbox; skipping OS exercise"
                );
                None
            }
            Err(error) => panic!("bind receiver: {error}"),
        }
    }

    #[test]
    fn real_unix_datagram_round_trip_has_private_permissions() {
        let (base, endpoint) = isolated_endpoint();
        let Some(receiver) = bind_or_skip(endpoint.clone()) else {
            cleanup(&base);
            return;
        };
        let directory_mode = fs::metadata(endpoint.directory_path())
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777;
        let socket_mode = fs::metadata(endpoint.socket_path())
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(socket_mode, 0o600);

        let mut publisher = Publisher::new(endpoint).expect("publisher");
        assert!(matches!(
            publisher.publish(RawInput::Started),
            PublishOutcome::Sent { ordinal: 1, .. }
        ));
        assert_eq!(
            receiver.receive().expect("receive").input,
            RawInput::Started
        );
        drop(receiver);
        cleanup(&base);
    }

    #[test]
    fn active_receiver_is_not_replaced() {
        let (base, endpoint) = isolated_endpoint();
        let Some(receiver) = bind_or_skip(endpoint.clone()) else {
            cleanup(&base);
            return;
        };
        assert!(matches!(
            Receiver::bind(endpoint),
            Err(EndpointError::EndpointInUse(_))
        ));
        drop(receiver);
        cleanup(&base);
    }

    #[test]
    fn stale_socket_is_removed_and_recreated() {
        let (base, endpoint) = isolated_endpoint();
        endpoint.prepare_for_bind().expect("prepare directory");
        let stale = match UnixDatagram::bind(endpoint.socket_path()) {
            Ok(stale) => stale,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!(
                    "Unix socket creation is denied by this test sandbox; skipping OS exercise"
                );
                cleanup(&base);
                return;
            }
            Err(error) => panic!("bind stale socket: {error}"),
        };
        drop(stale);
        assert!(endpoint.socket_path().exists());

        let receiver = bind_or_skip(endpoint.clone()).expect("socket was already created");
        assert!(endpoint.socket_path().exists());
        drop(receiver);
        assert!(!endpoint.socket_path().exists());
        cleanup(&base);
    }

    #[test]
    fn receiver_interruption_becomes_an_observable_ordinal_gap() {
        let (base, endpoint) = isolated_endpoint();
        let Some(first_receiver) = bind_or_skip(endpoint.clone()) else {
            cleanup(&base);
            return;
        };
        let mut publisher = Publisher::new(endpoint.clone()).expect("publisher");
        assert!(matches!(
            publisher.publish(RawInput::Started),
            PublishOutcome::Sent { ordinal: 1, .. }
        ));
        let first = first_receiver.receive().expect("first packet");
        drop(first_receiver);

        assert!(matches!(
            publisher.publish(RawInput::Paused),
            PublishOutcome::DroppedReceiverAbsent { ordinal: 2 }
        ));
        let second_receiver = bind_or_skip(endpoint).expect("socket support already established");
        assert!(matches!(
            publisher.publish(RawInput::Started),
            PublishOutcome::Sent { ordinal: 3, .. }
        ));
        let third = second_receiver.receive().expect("third packet");

        let mut continuity = ContinuityTracker::default();
        assert!(!continuity.observe(first).requires_boundary);
        let gap = continuity.observe(third);
        assert!(gap.requires_boundary);
        assert!(matches!(
            gap.diagnostic,
            Some(ContinuityDiagnostic::Gap { missing: 1, .. })
        ));
        drop(second_receiver);
        cleanup(&base);
    }
}
