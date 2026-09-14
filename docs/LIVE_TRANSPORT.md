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
implemented platform backend is Unix-domain datagrams for macOS and Linux.
Windows remains a first-class target, but its native local-IPC backend is not
implemented or validated by this macOS milestone; the crate boundary keeps that
backend replaceable without changing the wire envelope or estimator pipeline.

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

Run the host-native Unix companion manually:

```bash
cargo run -p adaptive-eta-runtime
```

It binds before or after ETS2, waits in a blocking receive with a one-second
timeout, remains alive while the game is absent, and prints status at most about
once per second while receiving. Status includes source epoch, normalized frame
count, stock/adaptive ETA when available, display factor, confidence, accepted
sample count, and detected missing transport messages. Idle, continuity,
malformed, unsupported-version, and clean-shutdown states are distinct.

SIGINT or SIGTERM requests a clean shutdown and socket cleanup. Abrupt process
termination is recovered by stale-endpoint handling on the next start.

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

## Future persistence boundary

The next milestone may load estimator state before runtime ingestion and save
accepted estimator changes after processing. It must not require changes to the
plugin, wire protocol, callback assembler, adapter timing semantics, or core
calibration mathematics.

Remaining live risks are the behavior of the socket send buffer under sustained
receiver starvation, real callback-path cost in ETS2, exact clean transported
parity, and observed reacquisition across intentional runtime/game restarts.
Those require the Tests A-E live protocol; automated evidence alone does not
claim the production path is fully live-validated. The Windows local-transport
backend and Windows/Linux target builds also remain explicit platform work; no
Windows or Linux validation is claimed here.
