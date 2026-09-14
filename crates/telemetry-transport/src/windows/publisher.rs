#![allow(unsafe_code)]

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use telemetry_adapter::RawInput;
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    GENERIC_WRITE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject};

use crate::handle::OwnedHandle;
use crate::{
    Endpoint, EndpointError, Envelope, MAX_PACKET_SIZE, PublishCounters, PublishOutcome,
    SenderInstance, encode,
};

const WRITE_WAIT_MS: u32 = 20;

#[derive(Clone, Copy)]
struct Packet {
    bytes: [u8; MAX_PACKET_SIZE],
    size: usize,
}

#[derive(Debug, Default)]
struct AtomicCounters {
    successful: AtomicU64,
    successful_bytes: AtomicU64,
    maximum_packet_bytes: AtomicUsize,
    would_block_drops: AtomicU64,
    receiver_absent_drops: AtomicU64,
    other_failures: AtomicU64,
}

#[derive(Debug)]
pub struct Publisher {
    sender_instance: SenderInstance,
    next_ordinal: u64,
    buffer: [u8; MAX_PACKET_SIZE],
    attempted: u64,
    serialization_failures: u64,
    callback_would_block_drops: u64,
    callback_other_failures: u64,
    queue: Option<SyncSender<Packet>>,
    worker: Option<JoinHandle<()>>,
    worker_counters: Arc<AtomicCounters>,
}

impl Publisher {
    /// Starts one process-lifetime transport worker and a single-slot bounded handoff.
    ///
    /// # Errors
    ///
    /// Returns an error only if the worker thread cannot be created. Receiver
    /// absence is normal and never prevents publisher construction.
    pub fn new(endpoint: Endpoint) -> Result<Self, EndpointError> {
        let (queue, receiver) = sync_channel(1);
        let worker_counters = Arc::new(AtomicCounters::default());
        let counters = Arc::clone(&worker_counters);
        let worker = std::thread::Builder::new()
            .name("adaptive-eta-pipe".to_owned())
            .spawn(move || worker_loop(endpoint, receiver, &counters))?;
        Ok(Self {
            sender_instance: SenderInstance::new_process(),
            next_ordinal: 1,
            buffer: [0; MAX_PACKET_SIZE],
            attempted: 0,
            serialization_failures: 0,
            callback_would_block_drops: 0,
            callback_other_failures: 0,
            queue: Some(queue),
            worker: Some(worker),
            worker_counters,
        })
    }

    #[must_use]
    pub const fn sender(&self) -> SenderInstance {
        self.sender_instance
    }

    #[must_use]
    pub fn counters(&self) -> PublishCounters {
        PublishCounters {
            attempted: self.attempted,
            successful: self.worker_counters.successful.load(Ordering::Relaxed),
            successful_bytes: self
                .worker_counters
                .successful_bytes
                .load(Ordering::Relaxed),
            maximum_packet_bytes: self
                .worker_counters
                .maximum_packet_bytes
                .load(Ordering::Relaxed),
            would_block_drops: self.callback_would_block_drops.saturating_add(
                self.worker_counters
                    .would_block_drops
                    .load(Ordering::Relaxed),
            ),
            receiver_absent_drops: self
                .worker_counters
                .receiver_absent_drops
                .load(Ordering::Relaxed),
            serialization_failures: self.serialization_failures,
            other_failures: self
                .callback_other_failures
                .saturating_add(self.worker_counters.other_failures.load(Ordering::Relaxed)),
        }
    }

    #[must_use]
    pub fn publish(&mut self, input: RawInput) -> PublishOutcome {
        let ordinal = self.next_ordinal;
        self.next_ordinal = self.next_ordinal.checked_add(1).unwrap_or(0);
        self.attempted = self.attempted.saturating_add(1);
        if ordinal == 0 {
            self.serialization_failures = self.serialization_failures.saturating_add(1);
            return PublishOutcome::SerializationFailed { ordinal };
        }
        let Ok(size) = encode(
            Envelope {
                sender: self.sender_instance,
                ordinal,
                input,
            },
            &mut self.buffer,
        ) else {
            self.serialization_failures = self.serialization_failures.saturating_add(1);
            return PublishOutcome::SerializationFailed { ordinal };
        };
        let packet = Packet {
            bytes: self.buffer,
            size,
        };
        let Some(queue) = &self.queue else {
            self.callback_other_failures = self.callback_other_failures.saturating_add(1);
            return PublishOutcome::OtherFailure {
                ordinal,
                kind: io::ErrorKind::BrokenPipe,
            };
        };
        match queue.try_send(packet) {
            Ok(()) => PublishOutcome::Pending {
                ordinal,
                bytes: size,
            },
            Err(TrySendError::Full(_)) => {
                self.callback_would_block_drops = self.callback_would_block_drops.saturating_add(1);
                PublishOutcome::DroppedWouldBlock { ordinal }
            }
            Err(TrySendError::Disconnected(_)) => {
                self.callback_other_failures = self.callback_other_failures.saturating_add(1);
                PublishOutcome::OtherFailure {
                    ordinal,
                    kind: io::ErrorKind::BrokenPipe,
                }
            }
        }
    }

    #[must_use]
    pub fn shutdown(mut self) -> PublishCounters {
        self.stop_worker();
        self.counters()
    }

    fn stop_worker(&mut self) {
        self.queue.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

#[allow(clippy::needless_pass_by_value)]
fn worker_loop(
    endpoint: Endpoint,
    receiver: std::sync::mpsc::Receiver<Packet>,
    counters: &AtomicCounters,
) {
    // SAFETY: null security/name pointers request a private unnamed manual-reset event.
    let Some(event) =
        OwnedHandle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })
    else {
        counters.other_failures.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let mut pipe: Option<OwnedHandle> = None;
    while let Ok(packet) = receiver.recv() {
        if pipe.is_none() {
            match open_pipe(&endpoint) {
                Ok(handle) => pipe = Some(handle),
                Err(OpenError::Absent) => {
                    counters
                        .receiver_absent_drops
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Err(OpenError::Other) => {
                    counters.other_failures.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            }
        }
        let outcome = write_packet(pipe.as_ref().expect("pipe established"), &event, packet);
        match outcome {
            WriteOutcome::Sent(bytes) => {
                counters.successful.fetch_add(1, Ordering::Relaxed);
                counters
                    .successful_bytes
                    .fetch_add(u64::from(bytes), Ordering::Relaxed);
                counters.maximum_packet_bytes.fetch_max(
                    usize::try_from(bytes).expect("u32 fits usize"),
                    Ordering::Relaxed,
                );
            }
            WriteOutcome::WouldBlock => {
                counters.would_block_drops.fetch_add(1, Ordering::Relaxed);
            }
            WriteOutcome::Disconnected => {
                counters
                    .receiver_absent_drops
                    .fetch_add(1, Ordering::Relaxed);
                pipe = None;
            }
            WriteOutcome::Other => {
                counters.other_failures.fetch_add(1, Ordering::Relaxed);
                pipe = None;
            }
        }
    }
}

enum OpenError {
    Absent,
    Other,
}

fn open_pipe(endpoint: &Endpoint) -> Result<OwnedHandle, OpenError> {
    // SAFETY: the endpoint owns a NUL-terminated UTF-16 name. No `WaitNamedPipe`
    // call is made; absence and busy instances fail immediately.
    let handle = unsafe {
        CreateFileW(
            endpoint.pipe_name_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    let Some(handle) = OwnedHandle::new(handle) else {
        let code = crate::handle::error_code();
        return if matches!(code, ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY) {
            Err(OpenError::Absent)
        } else {
            Err(OpenError::Other)
        };
    };
    endpoint
        .validate_server(handle.raw())
        .map_err(|_| OpenError::Other)?;
    Ok(handle)
}

enum WriteOutcome {
    Sent(u32),
    WouldBlock,
    Disconnected,
    Other,
}

fn write_packet(pipe: &OwnedHandle, event: &OwnedHandle, packet: Packet) -> WriteOutcome {
    // SAFETY: `event` is a valid manual-reset event.
    unsafe {
        let _ = ResetEvent(event.raw());
    }
    let mut overlapped = OVERLAPPED {
        hEvent: event.raw(),
        ..OVERLAPPED::default()
    };
    let mut transferred = 0_u32;
    // SAFETY: all handles and pointers remain valid until synchronous completion
    // or until cancellation has been observed below.
    let started = unsafe {
        WriteFile(
            pipe.raw(),
            packet.bytes.as_ptr(),
            u32::try_from(packet.size).expect("packet size fits u32"),
            &raw mut transferred,
            &raw mut overlapped,
        )
    };
    if started != 0 {
        return exact_write(transferred, packet.size);
    }
    let code = crate::handle::error_code();
    if code != ERROR_IO_PENDING {
        return classify_write_error(code);
    }
    // SAFETY: this waits only on the worker thread, for a fixed 20 ms maximum.
    match unsafe { WaitForSingleObject(event.raw(), WRITE_WAIT_MS) } {
        WAIT_OBJECT_0 => {
            // SAFETY: the event signalled completion for this exact operation.
            if unsafe {
                GetOverlappedResult(pipe.raw(), &raw const overlapped, &raw mut transferred, 0)
            } != 0
            {
                exact_write(transferred, packet.size)
            } else {
                classify_write_error(crate::handle::error_code())
            }
        }
        WAIT_TIMEOUT => match cancel_and_observe(pipe.raw(), &overlapped) {
            Ok(transferred) => exact_write(transferred, packet.size),
            Err(code) if code == windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED => {
                WriteOutcome::WouldBlock
            }
            Err(code) => classify_write_error(code),
        },
        _ => match cancel_and_observe(pipe.raw(), &overlapped) {
            Ok(transferred) => exact_write(transferred, packet.size),
            Err(code) => classify_write_error(code),
        },
    }
}

fn cancel_and_observe(pipe: HANDLE, overlapped: &OVERLAPPED) -> Result<u32, u32> {
    let mut transferred = 0_u32;
    // SAFETY: cancellation targets this worker's exact operation, then the
    // blocking result wait guarantees the stack buffer is no longer borrowed by Windows.
    unsafe {
        let _ = CancelIoEx(pipe, overlapped);
        if GetOverlappedResult(pipe, overlapped, &raw mut transferred, 1) != 0 {
            Ok(transferred)
        } else {
            Err(crate::handle::error_code())
        }
    }
}

fn exact_write(transferred: u32, expected: usize) -> WriteOutcome {
    if usize::try_from(transferred).expect("u32 fits usize") == expected {
        WriteOutcome::Sent(transferred)
    } else {
        WriteOutcome::Other
    }
}

fn classify_write_error(code: u32) -> WriteOutcome {
    if matches!(code, ERROR_BROKEN_PIPE | ERROR_NO_DATA) {
        WriteOutcome::Disconnected
    } else {
        WriteOutcome::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_receiver_never_blocks_or_retries_in_callback() {
        let endpoint = Endpoint::for_current_user().expect("endpoint");
        let mut publisher = Publisher::new(endpoint).expect("publisher");
        assert!(matches!(
            publisher.publish(RawInput::Paused),
            PublishOutcome::Pending { ordinal: 1, .. }
        ));
        let counters = publisher.shutdown();
        assert_eq!(counters.attempted, 1);
        assert_eq!(counters.receiver_absent_drops, 1);
    }

    #[test]
    fn bounded_handoff_drops_instead_of_growing() {
        let (queue, _receiver) = sync_channel(1);
        let packet = Packet {
            bytes: [0; MAX_PACKET_SIZE],
            size: 32,
        };
        queue.try_send(packet).expect("first slot");
        assert!(matches!(queue.try_send(packet), Err(TrySendError::Full(_))));
    }
}
