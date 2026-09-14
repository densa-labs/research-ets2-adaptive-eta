# Production live telemetry transport

This milestone moves the already validated `RawInput` stream across a process
boundary without changing adapter or calibration mathematics.

```text
ETS2
  -> SCS telemetry plugin
  -> non-blocking platform-local transport
  -> Adaptive ETA companion runtime
  -> TelemetryAdapter
  -> CalibrationCore
  -> ephemeral live Adaptive ETA state
```

Persistence, profile identity, management UI, overlay, installation lifecycle,
and ETA-accuracy history are deliberately not part of this boundary.

The protocol, continuity tracker, and runtime processor are shared Rust. The
platform backends are Unix-domain datagrams on macOS/Linux and Windows named
pipes on Windows. Both feed the same binary decoder and continuity tracker;
neither adapter nor calibration code contains platform transport behavior.

## Ownership

The plugin owns SCS callback registration, the validated frame/lifecycle
assembler, source epoch and frame sequence, `RawInput` creation, and a small
non-blocking publisher. A normal production build does not execute
`TelemetryAdapter` or `CalibrationCore`. The `developer-parity` feature retains
the opt-in in-process pipeline, JSONL capture, callback trace, and structured
live trace used to prove clean transport equality.

`telemetry-transport` owns the wire envelope, fixed binary codec, local endpoint,
publisher/receiver behavior, continuity tracking, and transport diagnostics. It
does not know calibration rules.

`adaptive-eta-runtime` owns the receiver, continuity-to-boundary handoff,
`TelemetryAdapter`, `CalibrationCore`, current in-memory estimator state, and
throttled developer status. State begins at the existing baseline on every
runtime launch and is not saved.

## Unix endpoint and permissions

On macOS, and as a Linux fallback, the endpoint is:

```text
/tmp/adaptive-eta-<uid>/telemetry-v1.sock
```

On Linux, `$XDG_RUNTIME_DIR/adaptive-eta-<uid>/telemetry-v1.sock` is preferred
when `XDG_RUNTIME_DIR` is available.

The fallback path is short enough for `sockaddr_un`. The UID comes from ownership
of the current user's home directory; no personal identifier is placed on the
wire. The runtime directory is a real, current-user-owned directory with mode
`0700` or stricter. A pre-existing symlink, non-directory, foreign owner, or
group/world-accessible directory is rejected rather than followed or repaired.
The bound socket is current-user-owned and set to `0600`.

At startup, an existing socket path must be a current-user-owned Unix socket.
If a probe reaches an active receiver, startup refuses to replace it. A
connection-refused socket is treated as stale and removed before binding. On a
clean shutdown, the receiver removes its owned socket. After a crash, the same
validated stale-socket procedure recovers it. This directory contains no
durable data.

## Protocol v1

Every datagram is independently bounded and contains:

```text
magic                 4 bytes  "AETA"
protocol version      2 bytes  little-endian, exactly 1
RawInput kind         1 byte
reserved              1 byte   must be zero
sender instance      16 bytes
transport ordinal     8 bytes  little-endian, nonzero
kind-specific payload
```

The sender instance is a per-plugin-process, non-persistent 128-bit session
token. The transport ordinal starts at one and advances for every publication
attempt, including attempts dropped because the runtime is absent or the socket
would block. It is intentionally independent of the frame sequence carried in
`RawFrame`, because lifecycle values also need loss detection.

The encoding is a small manually specified fixed-width binary format. It adds
no serializer or IPC framework to the game process. Lifecycle packets are 32
bytes, `SourceConnected` is 40 bytes, and a frame is 108 bytes. The hard receive
limit is 128 bytes. Frame option bits preserve unavailable SDK values; absent
numeric slots must contain canonical zero. Floating-point values are carried by
their exact IEEE-754 bit pattern.

Decoding rejects bad magic, ordinal zero, unknown input tags, nonzero reserved
fields, non-canonical absent values, wrong lengths, packets over 128 bytes, and
any protocol version other than 1. No packet field controls an allocation.
Malformed or unsupported packets are counted and reported without stopping the
runtime.

## Unix plugin hot path and loss policy

For each emitted `RawInput`, the production callback path:

1. reserves the next transport ordinal;
2. writes the envelope into one reusable 128-byte stack-owned buffer;
3. performs one non-blocking `send_to` to the fixed local socket path;
4. updates in-memory counters and returns.

There is no connect wait, retry loop, acknowledgement, queue, filesystem lock,
file write, UI work, network operation, or per-frame log. If the receiver does
not exist, the socket would block, or another send error occurs, that input is
dropped. Counters distinguish successful sends, receiver-absent drops,
would-block drops, serialization failures, and other failures; a compact summary
is logged only at plugin shutdown.

Because the ordinal advances before every attempted send, a later successful
packet exposes all callback-side drops to the receiver.

## Windows endpoint and security

The Windows endpoint is one named pipe scoped to the current Windows user SID
and logon session:

```text
\\.\pipe\DensaLabs.AdaptiveETA.Telemetry.v1.<user SID>.session-<session ID>
```

The SID and session ID are read from the current process token and Windows
session APIs. They are deterministic in both the ETS2 process and companion
runtime without a configuration file, while separating unrelated users and
concurrent Terminal Services sessions.

The runtime creates exactly one message-mode pipe instance with
`FILE_FLAG_FIRST_PIPE_INSTANCE` and `PIPE_REJECT_REMOTE_CLIENTS`. Its protected
DACL grants full access only to the current user SID and Local System. The
handle is non-inheritable. A second runtime cannot replace an active receiver;
when the owning process exits, Windows removes the pipe instance, so no
filesystem-style stale endpoint remains.

The plugin does not trust the pipe name alone. After opening a server, its
transport worker obtains the named-pipe server process ID and verifies that the
server process token has the expected user SID and session ID. A pipe created by
another logged-in user is closed and rejected even if that process deliberately
uses the expected textual name. Normal creation and use require no elevation.

## Windows plugin hot path and loss policy

The Windows callback path never calls `CreateFileW`, `WaitNamedPipe`,
`WriteFile`, a wait API, or reconnect logic. For every `RawInput` it:

1. reserves the next transport ordinal;
2. encodes protocol v1 into the publisher's reusable 128-byte buffer;
3. copies the fixed packet into a single preallocated `sync_channel(1)` slot
   using `try_send`;
4. updates in-memory counters and returns immediately.

There is one process-lifetime transport worker, not one worker per connection.
The single-slot handoff is the complete user-space queue: it cannot grow. If the
slot is occupied, the current publication is dropped immediately as
would-block. Ordinals advance before the handoff, so later delivery exposes the
drop exactly as on Unix.

The worker owns connection and reconnection. It never calls `WaitNamedPipe` and
does not run a retry or sleep loop. When a packet arrives with no open pipe, it
makes one `CreateFileW` attempt; absent/busy endpoints count as receiver-absent
and that packet is dropped. Later packets independently permit late runtime
attachment. Connected writes use overlapped I/O with a fixed 20 ms worker-only
deadline. A write that cannot complete is cancelled and observed before its
stack packet is released. The worker has no access to SCS state and can block
neither telemetry callbacks nor frame assembly.

The runtime creates a message-mode pipe, preserving one protocol packet per
native message without byte-stream framing. Its kernel inbound buffer request
is a bounded 4 KiB. The runtime uses overlapped connect/read operations with the
existing one-second idle timeout. It reads into exactly `MAX_PACKET_SIZE + 1`
bytes; the extra byte detects oversized messages. Truncated, oversized,
malformed, and unsupported-version messages flow to the same shared protocol
errors as Unix. An oversized message disconnects that client so an unread tail
cannot be confused with another packet.

When a runtime exits, a connected worker observes a broken pipe, drops the
affected ordinal, and returns to disconnected state. A later runtime accepts a
new connection. When a plugin exits, the runtime observes the client disconnect
and resumes listening on the same server handle. A new plugin has a new sender
instance, so the shared tracker emits `NewSender`. There is no transport-level
session inference or platform-specific timestamping.

## Continuity and recovery

Sequential ordinals from the current sender are accepted. A duplicate is
ignored and never reaches the adapter. An older out-of-order ordinal is ignored
and conservatively causes a timing boundary. A forward gap causes a timing
boundary before the received input is processed. A new sender causes a boundary
before its first input, and retired senders are ignored rather than reactivated.

The boundary is the existing adapter-facing `TimingDiscontinuity`. It resets
the adapter interval clock and navigation availability, discards any open core
sample, and requires normal stabilization again. Intermediate frames are never
fabricated and missing time is never integrated.

A newly launched runtime may first observe any ordinal because its estimator
and interval clock are also new. Its first frame establishes a baseline and the
core stabilizes normally. This makes late companion startup and runtime restart
safe without requiring ETS2 to restart. Conversely, a new plugin process has a
new sender instance; a runtime left alive across an ETS2 restart recognizes it
and cannot continue the previous window.

## Runtime operation and observability

Run the host-native companion manually:

```bash
cargo run -p adaptive-eta-runtime
```

It binds before or after ETS2, waits in a blocking receive with a one-second
timeout, remains alive while the game is absent, and prints status at most about
once per second while receiving. Status includes source epoch, normalized frame
count, stock/adaptive ETA when available, display factor, confidence, accepted
sample count, and detected missing transport messages. Idle, continuity,
malformed, unsupported-version, and clean-shutdown states are distinct.

On Unix, SIGINT or SIGTERM requests a clean shutdown and socket cleanup. On
Windows, console Ctrl+C/Ctrl+Break/close requests the same clean runtime exit.
Abrupt Unix termination is recovered by stale-endpoint handling; abrupt Windows
termination releases the named-pipe kernel object automatically.

No raw telemetry is stored by default. For a bounded developer parity run only,
the runtime can retain and write the existing structured trace equality surface:

```bash
cargo run -p adaptive-eta-runtime -- --trace /absolute/path/transport.trace.json
```

Build the plugin with
`scripts/build-macos-spike.sh --developer-parity`, enable the existing capture
marker, and compare the plugin's JSONL capture with the runtime trace using the
existing parity command. With uninterrupted delivery, the expected result
remains exact `MATCH`; no epsilon comparison is introduced. Rebuild without the
flag afterward to restore the normal production plugin artifact.

## Validation procedure

1. Start the runtime, launch ETS2, drive, and confirm live state updates with no
   transport diagnostics.
2. Launch ETS2 without the runtime, drive briefly, start the runtime, and confirm
   it establishes a fresh baseline and later stabilizes without restarting ETS2.
3. While driving, stop the runtime, continue briefly, restart it, and confirm
   ETS2 remains responsive and sampling later resumes without a cross-gap sample.
4. Leave the runtime active while quitting and relaunching ETS2; confirm the new
   sender/source is bounded and later stabilizes.
5. For a short uninterrupted run, use `developer-parity` plus `--trace` and
   require the existing exact parity tool to print `MATCH records=<count>`.

Automated tests cover codec round trips and rejection, sequence anomalies,
sender replacement, absent receivers, real Unix datagrams and permissions,
active/stale endpoints, gap-to-core boundary behavior, and recovery. The real
socket exercises may report a skip in a sandbox that forbids Unix socket
creation; run them outside that sandbox for OS-level evidence.

### Native Windows validation

On a native 64-bit Windows host with the Rust MSVC target/toolchain installed:

```powershell
cargo test -p telemetry-transport --all-features --target x86_64-pc-windows-msvc
cargo test -p adaptive-eta-runtime --all-features --target x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
cargo clippy --workspace --all-targets --all-features --target x86_64-pc-windows-msvc -- -D warnings
```

The Windows-only transport suite exercises:

```text
W1 receiver first and complete message delivery
W2 publisher first, receiver absent, and late attachment
W3 receiver restart and observable ordinal gap
W4 publisher restart and NewSender
W5 100,000 callback publications through the bounded handoff
W6 truncated, bad-magic, oversized, unsupported-version, duplicate,
   out-of-order, and ordinal-gap input
single-active-receiver enforcement and endpoint release on shutdown
```

For real ETS2 validation, install the resulting CDylib from
`target\x86_64-pc-windows-msvc\release` into a test ETS2
`bin\win_x64\plugins` directory under the eventual product filename
`AdaptiveETA.dll`. Repeat transport Tests A-D with the Windows runtime and use
the existing developer parity capture/trace flow for exact Test E equality.
Artifact renaming and installation lifecycle remain packaging work, not part of
the transport crate.

Cross-target checking proves Windows API and type selection only. A native MSVC
linker is still required to produce the DLL and runtime executable. Until the
Windows-only suite and ETS2 are actually executed on Windows, native named-pipe
behavior and live SCS loading remain explicitly pending rather than inferred
from a target check.

## Future persistence boundary

The next milestone may load estimator state before runtime ingestion and save
accepted estimator changes after processing. It must not require changes to the
plugin, wire protocol, callback assembler, adapter timing semantics, or core
calibration mathematics.

The Unix backend has completed real ETS2 Tests A-E on macOS, including exact
`MATCH records=7556`. The Windows backend is implemented and can be
cross-compiled from macOS, but its Windows-only tests and real ETS2 loading must
still be run on a native Windows environment. Linux target compilation is not
equivalent to native Linux socket or ETS2 execution either.
