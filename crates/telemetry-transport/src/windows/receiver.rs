#![allow(unsafe_code)]

use std::io;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_CONNECTED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND, ReadFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

use crate::handle::OwnedHandle;
use crate::{
    Endpoint, EndpointError, Envelope, MAX_PACKET_SIZE, ProtocolError, ReceiveError, decode,
};

const PIPE_BUFFER_BYTES: u32 = 4096;

#[derive(Debug)]
pub struct Receiver {
    pipe: OwnedHandle,
    event: OwnedHandle,
    endpoint: Endpoint,
    timeout: Duration,
    connected: bool,
}

impl Receiver {
    /// Creates the single local-only, current-user named-pipe receiver.
    ///
    /// # Errors
    ///
    /// Returns an error if identity/ACL construction fails or another receiver
    /// already owns the first pipe instance.
    pub fn bind(endpoint: Endpoint) -> Result<Self, EndpointError> {
        let mut descriptor = endpoint.security_descriptor()?;
        let attributes = descriptor.attributes();
        // SAFETY: all pointers refer to live, initialized values for the duration
        // of `CreateNamedPipeW`; Windows copies the security descriptor.
        let pipe = OwnedHandle::new(unsafe {
            CreateNamedPipeW(
                endpoint.pipe_name_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                PIPE_BUFFER_BYTES,
                0,
                &raw const attributes,
            )
        })
        .ok_or_else(crate::handle::last_error)?;
        // SAFETY: null security/name pointers request a private unnamed manual-reset event.
        let event =
            OwnedHandle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
                .ok_or_else(crate::handle::last_error)?;
        Ok(Self {
            pipe,
            event,
            endpoint,
            timeout: Duration::from_secs(1),
            connected: false,
        })
    }

    /// Sets the maximum time spent waiting for one connect/read operation.
    ///
    /// # Errors
    ///
    /// This backend currently accepts every duration and returns `Ok(())`.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.timeout = timeout.unwrap_or(Duration::MAX);
        Ok(())
    }

    /// Receives and decodes one complete named-pipe message.
    ///
    /// # Errors
    ///
    /// Returns a typed timeout/I/O error or a bounded shared protocol error.
    pub fn receive(&mut self) -> Result<Envelope, ReceiveError> {
        for _ in 0..2 {
            if !self.connected {
                self.connect().map_err(ReceiveError::Io)?;
            }
            match self.read_message() {
                Ok(envelope) => return Ok(envelope),
                Err(ReadError::Disconnected) => self.disconnect(),
                Err(ReadError::Io(error)) => return Err(ReceiveError::Io(error)),
                Err(ReadError::Protocol(error)) => return Err(ReceiveError::Protocol(error)),
            }
        }
        Err(ReceiveError::Io(io::Error::new(
            io::ErrorKind::WouldBlock,
            "named-pipe peer disconnected",
        )))
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn connect(&mut self) -> io::Result<()> {
        self.reset_event()?;
        let mut overlapped = self.overlapped();
        // SAFETY: the pipe is a valid overlapped server handle and `overlapped`
        // remains live until completion/cancellation is observed.
        if unsafe { ConnectNamedPipe(self.pipe.raw(), &raw mut overlapped) } != 0 {
            self.connected = true;
            return Ok(());
        }
        match crate::handle::error_code() {
            ERROR_PIPE_CONNECTED => {
                self.connected = true;
                Ok(())
            }
            ERROR_IO_PENDING => {
                self.wait_for(&overlapped, None)?;
                self.connected = true;
                Ok(())
            }
            _ => Err(crate::handle::last_error()),
        }
    }

    fn read_message(&mut self) -> Result<Envelope, ReadError> {
        let mut buffer = [0_u8; MAX_PACKET_SIZE + 1];
        self.reset_event().map_err(ReadError::Io)?;
        let mut overlapped = self.overlapped();
        let mut transferred = 0_u32;
        // SAFETY: the buffer and overlap state remain live until completion or
        // cancellation is synchronously observed by `wait_for`.
        let started = unsafe {
            ReadFile(
                self.pipe.raw(),
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).expect("receive buffer fits u32"),
                &raw mut transferred,
                &raw mut overlapped,
            )
        };
        if started == 0 {
            match crate::handle::error_code() {
                ERROR_IO_PENDING => {
                    transferred = match self.wait_for(&overlapped, Some(&mut transferred)) {
                        Ok(transferred) => transferred,
                        Err(error)
                            if error
                                .raw_os_error()
                                .and_then(|code| u32::try_from(code).ok())
                                == Some(ERROR_MORE_DATA) =>
                        {
                            return self.oversized();
                        }
                        Err(error) => return Err(classify_read_io(error)),
                    };
                }
                ERROR_MORE_DATA => return self.oversized(),
                code if disconnected(code) => return Err(ReadError::Disconnected),
                _ => return Err(ReadError::Io(crate::handle::last_error())),
            }
        }
        let size = usize::try_from(transferred).expect("u32 fits usize");
        if size > MAX_PACKET_SIZE {
            return self.oversized();
        }
        decode(&buffer[..size]).map_err(ReadError::Protocol)
    }

    fn oversized(&mut self) -> Result<Envelope, ReadError> {
        self.disconnect();
        Err(ReadError::Protocol(ProtocolError::PacketTooLarge {
            actual: MAX_PACKET_SIZE + 1,
            maximum: MAX_PACKET_SIZE,
        }))
    }

    fn wait_for(&self, overlapped: &OVERLAPPED, output: Option<&mut u32>) -> io::Result<u32> {
        let timeout_ms = duration_ms(self.timeout);
        // SAFETY: this is the runtime side; waiting is bounded by the configured timeout.
        match unsafe { WaitForSingleObject(self.event.raw(), timeout_ms) } {
            WAIT_OBJECT_0 => {
                let mut transferred = 0_u32;
                // SAFETY: the event signalled this exact operation's completion.
                if unsafe {
                    GetOverlappedResult(self.pipe.raw(), overlapped, &raw mut transferred, 0)
                } == 0
                {
                    Err(crate::handle::last_error())
                } else {
                    if let Some(output) = output {
                        *output = transferred;
                    }
                    Ok(transferred)
                }
            }
            WAIT_TIMEOUT => match self.cancel_and_observe(overlapped) {
                Ok(transferred) => {
                    if let Some(output) = output {
                        *output = transferred;
                    }
                    Ok(transferred)
                }
                Err(code) if code == ERROR_OPERATION_ABORTED => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "named-pipe receive timed out",
                )),
                Err(code) => Err(crate::handle::error_from_code(code)),
            },
            _ => match self.cancel_and_observe(overlapped) {
                Ok(transferred) => {
                    if let Some(output) = output {
                        *output = transferred;
                    }
                    Ok(transferred)
                }
                Err(code) => Err(crate::handle::error_from_code(code)),
            },
        }
    }

    fn cancel_and_observe(&self, overlapped: &OVERLAPPED) -> Result<u32, u32> {
        let mut transferred = 0_u32;
        // SAFETY: cancellation targets this receiver's exact operation; waiting
        // for the cancellation makes stack buffers safe to release.
        unsafe {
            let _ = CancelIoEx(self.pipe.raw(), overlapped);
            if GetOverlappedResult(self.pipe.raw(), overlapped, &raw mut transferred, 1) != 0 {
                Ok(transferred)
            } else {
                Err(crate::handle::error_code())
            }
        }
    }

    fn reset_event(&self) -> io::Result<()> {
        // SAFETY: `event` is a valid manual-reset event.
        if unsafe { ResetEvent(self.event.raw()) } == 0 {
            Err(crate::handle::last_error())
        } else {
            Ok(())
        }
    }

    fn overlapped(&self) -> OVERLAPPED {
        OVERLAPPED {
            hEvent: self.event.raw(),
            ..OVERLAPPED::default()
        }
    }

    fn disconnect(&mut self) {
        // SAFETY: the handle is a named-pipe server owned by this receiver.
        unsafe {
            let _ = DisconnectNamedPipe(self.pipe.raw());
        }
        self.connected = false;
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.disconnect();
    }
}

enum ReadError {
    Disconnected,
    Io(io::Error),
    Protocol(ProtocolError),
}

fn classify_read_io(error: io::Error) -> ReadError {
    if error
        .raw_os_error()
        .and_then(|code| u32::try_from(code).ok())
        .is_some_and(disconnected)
    {
        ReadError::Disconnected
    } else {
        ReadError::Io(error)
    }
}

const fn disconnected(code: u32) -> bool {
    matches!(code, ERROR_BROKEN_PIPE | ERROR_NO_DATA)
}

fn duration_ms(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;
    use telemetry_adapter::RawInput;
    use windows_sys::Win32::Foundation::GENERIC_WRITE;
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING, WriteFile};

    use crate::{
        ContinuityDiagnostic, ContinuityTracker, Envelope, PublishOutcome, Publisher,
        SenderInstance, encode,
    };

    static TEST_ID: AtomicU64 = AtomicU64::new(1);

    fn endpoint() -> Endpoint {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        Endpoint::for_test(&format!("{}-{id}", std::process::id())).expect("test endpoint")
    }

    fn receive(receiver: &mut Receiver) -> Envelope {
        receiver
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        receiver.receive().expect("receive")
    }

    fn wait_for_counter(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !predicate() {
            assert!(Instant::now() < deadline, "counter did not advance");
            std::thread::yield_now();
        }
    }

    #[test]
    fn w1_receiver_first_delivers_complete_messages() {
        let endpoint = endpoint();
        let mut receiver = Receiver::bind(endpoint.clone()).expect("receiver");
        let mut publisher = Publisher::new(endpoint).expect("publisher");
        assert!(matches!(
            publisher.publish(RawInput::Started),
            PublishOutcome::Pending {
                ordinal: 1,
                bytes: 32
            }
        ));
        assert_eq!(receive(&mut receiver).input, RawInput::Started);
        let counters = publisher.shutdown();
        assert_eq!(counters.successful, 1);
    }

    #[test]
    fn w2_publisher_first_recovers_when_receiver_appears() {
        let endpoint = endpoint();
        let mut publisher = Publisher::new(endpoint.clone()).expect("publisher");
        let _ = publisher.publish(RawInput::Paused);
        wait_for_counter(|| publisher.counters().receiver_absent_drops == 1);

        let mut receiver = Receiver::bind(endpoint).expect("receiver");
        let _ = publisher.publish(RawInput::Started);
        let envelope = receive(&mut receiver);
        assert_eq!(envelope.ordinal, 2);
        assert_eq!(envelope.input, RawInput::Started);
        let counters = publisher.shutdown();
        assert_eq!(counters.receiver_absent_drops, 1);
        assert_eq!(counters.successful, 1);
    }

    #[test]
    fn w3_receiver_restart_exposes_an_ordinal_gap() {
        let endpoint = endpoint();
        let mut first_receiver = Receiver::bind(endpoint.clone()).expect("first receiver");
        let mut publisher = Publisher::new(endpoint.clone()).expect("publisher");
        let _ = publisher.publish(RawInput::Started);
        let first = receive(&mut first_receiver);
        drop(first_receiver);

        let _ = publisher.publish(RawInput::Paused);
        wait_for_counter(|| publisher.counters().receiver_absent_drops >= 1);
        let mut second_receiver = Receiver::bind(endpoint).expect("second receiver");
        let _ = publisher.publish(RawInput::Started);
        let resumed = receive(&mut second_receiver);

        let mut continuity = ContinuityTracker::default();
        assert!(!continuity.observe(first).requires_boundary);
        let outcome = continuity.observe(resumed);
        assert!(outcome.requires_boundary);
        assert!(matches!(
            outcome.diagnostic,
            Some(ContinuityDiagnostic::Gap { missing: 1, .. })
        ));
    }

    #[test]
    fn w4_publisher_restart_is_a_new_sender() {
        let endpoint = endpoint();
        let mut receiver = Receiver::bind(endpoint.clone()).expect("receiver");
        let mut first_publisher = Publisher::new(endpoint.clone()).expect("first publisher");
        let _ = first_publisher.publish(RawInput::Started);
        let first = receive(&mut receiver);
        let _ = first_publisher.shutdown();

        let publisher_thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            let mut second_publisher = Publisher::new(endpoint).expect("second publisher");
            let _ = second_publisher.publish(RawInput::Started);
            second_publisher.shutdown()
        });
        let second = receive(&mut receiver);
        let _ = publisher_thread.join().expect("publisher thread");
        let mut continuity = ContinuityTracker::default();
        let _ = continuity.observe(first);
        let outcome = continuity.observe(second);
        assert!(outcome.requires_boundary);
        assert!(matches!(
            outcome.diagnostic,
            Some(ContinuityDiagnostic::NewSender { .. })
        ));
    }

    #[test]
    fn w5_high_rate_publication_remains_bounded_and_responsive() {
        let endpoint = endpoint();
        let _receiver = Receiver::bind(endpoint.clone()).expect("receiver");
        let mut publisher = Publisher::new(endpoint).expect("publisher");
        let started = Instant::now();
        for _ in 0..100_000 {
            let _ = publisher.publish(RawInput::Paused);
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        let counters = publisher.shutdown();
        assert_eq!(counters.attempted, 100_000);
        assert!(counters.would_block_drops > 0);
    }

    #[test]
    fn w6_invalid_and_continuity_inputs_do_not_poison_receiver() {
        let endpoint = endpoint();
        let mut receiver = Receiver::bind(endpoint.clone()).expect("receiver");
        let client = RawClient::open(&endpoint);

        client.send(b"bad");
        assert!(matches!(
            receiver.receive(),
            Err(ReceiveError::Protocol(ProtocolError::PacketTooShort { .. }))
        ));

        let valid = encoded(1, RawInput::Paused);
        let mut bad_magic = valid.clone();
        bad_magic[0] = b'X';
        client.send(&bad_magic);
        assert!(matches!(
            receiver.receive(),
            Err(ReceiveError::Protocol(ProtocolError::InvalidMagic))
        ));

        let mut unsupported = valid.clone();
        unsupported[4..6].copy_from_slice(&2_u16.to_le_bytes());
        client.send(&unsupported);
        assert!(matches!(
            receiver.receive(),
            Err(ReceiveError::Protocol(ProtocolError::UnsupportedVersion {
                found: 2
            }))
        ));

        client.send(&[0; MAX_PACKET_SIZE + 1]);
        assert!(matches!(
            receiver.receive(),
            Err(ReceiveError::Protocol(ProtocolError::PacketTooLarge { .. }))
        ));
        drop(client);

        let writer_endpoint = endpoint.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            let client = RawClient::open(&writer_endpoint);
            client.send(&encoded(1, RawInput::Paused));
            client.send(&encoded(1, RawInput::Paused));
            client.send(&encoded(4, RawInput::Started));
            client.send(&encoded(3, RawInput::Paused));
        });
        let first = receive(&mut receiver);
        let duplicate = receive(&mut receiver);
        let gap = receive(&mut receiver);
        let out_of_order = receive(&mut receiver);
        writer.join().expect("raw writer");

        let mut continuity = ContinuityTracker::default();
        assert!(!continuity.observe(first).requires_boundary);
        assert!(matches!(
            continuity.observe(duplicate).diagnostic,
            Some(ContinuityDiagnostic::Duplicate { .. })
        ));
        assert!(matches!(
            continuity.observe(gap).diagnostic,
            Some(ContinuityDiagnostic::Gap { .. })
        ));
        assert!(matches!(
            continuity.observe(out_of_order).diagnostic,
            Some(ContinuityDiagnostic::OutOfOrder { .. })
        ));
    }

    #[test]
    fn receiver_drop_releases_endpoint_for_clean_restart() {
        let endpoint = endpoint();
        let receiver = Receiver::bind(endpoint.clone()).expect("receiver");
        assert!(Receiver::bind(endpoint.clone()).is_err());
        drop(receiver);
        Receiver::bind(endpoint).expect("replacement receiver");
    }

    fn encoded(ordinal: u64, input: RawInput) -> Vec<u8> {
        let mut buffer = [0; MAX_PACKET_SIZE];
        let size = encode(
            Envelope {
                sender: SenderInstance([7; 16]),
                ordinal,
                input,
            },
            &mut buffer,
        )
        .expect("encode");
        buffer[..size].to_vec()
    }

    struct RawClient {
        pipe: OwnedHandle,
    }

    impl RawClient {
        fn open(endpoint: &Endpoint) -> Self {
            // SAFETY: the endpoint contains a valid NUL-terminated name.
            let pipe = OwnedHandle::new(unsafe {
                CreateFileW(
                    endpoint.pipe_name_ptr(),
                    GENERIC_WRITE,
                    0,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    0,
                    std::ptr::null_mut(),
                )
            })
            .expect("open raw client");
            endpoint
                .validate_server(pipe.raw())
                .expect("server identity");
            Self { pipe }
        }

        fn send(&self, bytes: &[u8]) {
            let mut transferred = 0_u32;
            // SAFETY: this is a synchronous write from a live byte slice.
            assert_ne!(
                unsafe {
                    WriteFile(
                        self.pipe.raw(),
                        bytes.as_ptr(),
                        u32::try_from(bytes.len()).expect("test packet fits u32"),
                        &raw mut transferred,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            assert_eq!(
                usize::try_from(transferred).expect("u32 fits usize"),
                bytes.len()
            );
        }
    }
}
