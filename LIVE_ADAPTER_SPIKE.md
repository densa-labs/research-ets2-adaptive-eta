# Live adapter integration and replay parity

This developer spike connects the real SCS callback stream to the existing
`RawInput -> TelemetryAdapter -> CalibrationEngine` path. It does not add a
production transport or persistent player data.

## Callback bridge contract

The plugin subscribes to frame start/end, paused, started, configuration, and
gameplay events. It also subscribes with `EACH_FRAME | NO_VALUE` to local scale,
game time, speed, odometer, navigation distance, and navigation time.

At `frame_start`, the bridge opens a new frame containing the pause-aware SDK
timestamp and timer-restart flag. Each channel callback updates only that open
frame. At `frame_end`, one `RawFrame` snapshot is emitted when channel callbacks
were delivered. Channel storage is reset at every frame start: an explicit SDK
no-value is retained as `None`, while a callback missing from an otherwise
active frame is also `None` and raises an `IncompleteFrame` diagnostic. No prior
value is substituted.

The real run showed that ETS2 emits frame pairs without any channel callbacks
during startup/loading and while paused. Those expected empty frames are not
`RawFrame` observations and are suppressed. A timer-restart flag on such a
startup frame is retained as the existing `TimerRestart` lifecycle input.

The callback order used by the plugin is therefore preserved as follows:

```text
frame_start -> channel callbacks -> frame_end -> RawFrame
paused/started/configuration/gameplay -> immediate RawInput
```

Lifecycle callbacks that occur between frame callbacks are processed
immediately and retain that relative order. A frame start while another frame
is open, a channel outside a frame, or a frame end without a start produces a
typed bridge diagnostic. The optional callback trace records the real order so
these assumptions can be checked in ETS2 rather than inferred from SDK names.

The source epoch increments only when the plugin is initialized as a new source
session. Frame sequence starts at one and increases once per frame start within
that epoch. The frame's timer-restart flag is passed through directly; it is not
also emitted as a duplicate lifecycle record.

Job configuration with attributes maps to `JobChanged`; an empty job
configuration maps to `JobEnded`. Job cancelled/delivered gameplay events map
to `JobEnded`, and public ferry/train gameplay events map to their corresponding
existing lifecycle inputs. No save/load heuristic is added.

## Clock ownership

The plugin no longer integrates scaled time. The frame-end snapshot contains
the raw pause-aware timestamp and current scale. `TelemetryAdapter` alone owns
clock integration and applies its previously observed scale to the interval
ending at the new timestamp. Missing/invalid scale, pause behavior, timestamp
regression, and timer restart retain the adapter's existing behavior.

## Developer capture

Capture is opt-in through marker files so launching ETS2 from Steam does not
depend on shell environment variables. Enable a short capture with:

```bash
mkdir -p "$HOME/Library/Application Support/Adaptive ETA/dev"
touch "$HOME/Library/Application Support/Adaptive ETA/dev/capture.enabled"
```

For the high-volume ordered callback trace as well, create:

```bash
touch "$HOME/Library/Application Support/Adaptive ETA/dev/callback-trace.enabled"
```

Each session creates paired files named like:

```text
live-<unix-seconds>-<pid>.jsonl
live-<unix-seconds>-<pid>.trace.json
live-<unix-seconds>-<pid>.callbacks.log   # only with callback marker
```

The JSONL file is the existing record-version-1 format. Every `RawInput` is
written before the same value enters the live deterministic pipeline. Capture
uses buffered writes; adapter/core work remains independent. An open, encode,
write, flush, or trace-finalization failure is logged and invalidates that
session for parity, but does not alter live estimator mathematics.

Remove the marker files after the test so future developer builds do not
capture telemetry:

```bash
rm "$HOME/Library/Application Support/Adaptive ETA/dev/capture.enabled"
rm -f "$HOME/Library/Application Support/Adaptive ETA/dev/callback-trace.enabled"
```

## Exact parity surface

Live and replay share `DeterministicPipeline`. Its structured report contains,
per input ordinal, the ordered adapter outputs (normalized `EngineInput` values
and typed diagnostics) and ordered core decisions. The equality surface also
includes the summary, collected adapter diagnostics, final estimator state,
factor/confidence/display values, and final adaptive ETA when defined.

Trace version 1 serializes that report as JSON. `serde_json` uses its
`float_roundtrip` parser so difficult computed `f64` values return with the
same bits; this is covered by a captured-value regression test. Parity uses
Rust `PartialEq` without rounding or epsilon matching. Filenames, wall-clock
creation time, process ID, and paths are outside the report and therefore
outside equality.

Run parity after ETS2 shuts down cleanly and writes the trace:

```bash
cargo run -p telemetry-adapter --bin parity -- \
  "/path/to/live-SESSION.jsonl" \
  "/path/to/live-SESSION.trace.json"
```

Success prints `MATCH records=<count>` and exits zero. Failure prints
`MISMATCH`, the first differing input (with source epoch/sequence when the input
is a frame), and structured live/replay values.

## Manual live-validation procedure

1. Build and install the diagnostic plugin using the existing macOS scripts.
2. Create `capture.enabled`; create `callback-trace.enabled` for the first run.
3. Launch ETS2 normally through Steam. Load a profile and wait for navigation.
4. Drive normally, stop unpaused briefly, pause/resume, and deliberately miss a
   turn. If practical, load a save. Do not force unavailable-navigation or
   scale transitions through unsafe manipulation.
5. Quit ETS2 cleanly to flush the capture and live trace.
6. Remove both marker files.
7. Run the parity command above on the matching JSONL/trace pair.

For a mismatch, retain the JSONL, live trace, and optional callback log. The
parity output identifies the first divergence; no manual scan of the full trace
is required.

## Evidence status

The first real capture contained 27,759 inputs and reached exact `MATCH` after
enabling `serde_json`'s exact `float_roundtrip` parser. The initial comparison
identified a one-ULP trace decode difference at input 386; the trace file and
live computation were correct, and the parser feature plus a regression test
fixed the representation loss without weakening equality.

Observed callback structure was exact on all 23,032 active frames:

```text
frame_start
local.scale
game.time
truck.speed
truck.odometer
truck.navigation.distance
truck.navigation.time
frame_end
```

Another 4,711 startup/paused frames contained no channel callbacks. No active
partial frame, channel outside a frame, or nested/missing frame boundary was
observed. Five paused events and five started events occurred between complete
frames. The pause-aware timestamp stayed fixed during each pause and advanced
16,666 microseconds on the first resumed frame. Active local scale was always
available and changed ten times between 3 and 19, including rapid oscillation.
Both navigation values were available together on every active frame. The run
also exercised reroute-like navigation jumps, a same-session save load, job
configuration changes, five accepted calibration samples, and clean source
shutdown.

The corrected build then completed a follow-up capture with exact parity:

```text
MATCH records=7812
```

That capture contained 7,801 emitted `RawFrame` values, all with the complete
six-channel callback sequence and paired navigation availability. The callback
trace also contained 2,369 channel-less startup/paused frame pairs; all were
suppressed as intended, producing sequence gaps that remained safe across the
explicit lifecycle boundaries. No empty raw frame, bridge diagnostic, adapter
diagnostic, capture failure, or callback-order anomaly occurred. Two accepted
calibration samples were produced and shutdown finalized the trace cleanly.

The live adapter/replay-parity milestone is therefore validated for the
documented deterministic equality surface. Remaining work belongs to the next
production integration/transport milestone rather than this developer spike.
