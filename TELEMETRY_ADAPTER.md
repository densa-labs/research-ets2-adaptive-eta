# Normalized telemetry adapter and replay harness

This milestone connects raw-ish SCS telemetry concepts to `adaptive-eta-core`
without adding a live transport. It is an offline/developer interface, not the
production plugin protocol.

## Adapter contract

`TelemetryAdapter::process` accepts a `RawInput` and returns an ordered list of
typed `AdapterOutput` values. Inputs are source/lifecycle events or `RawFrame`:

```text
source epoch, sequence, paused simulation timestamp
local scale, game time
navigation distance/time with explicit availability
speed, odometer, timer-restart flag
```

Outputs are either an `adaptive_eta_core::EngineInput` or an
`AdapterDiagnostic`. The adapter validates and normalizes telemetry but never
owns or mutates estimator state. Source sequence and epoch values are preserved
so the core remains responsible for its existing sequence-gap, route-progress,
and odometer/navigation consistency policies.

Unavailable channels are represented as JSON `null`/Rust `None`; they are not
translated to zero. Both navigation fields must be available together. Missing
or malformed required speed/odometer/navigation data emits a typed diagnostic
and an `AdapterRejectedInput` boundary rather than a fabricated frame.

## Clock model

The first frame establishes a timing and scale baseline. For every subsequent
trusted, unpaused interval:

```text
deltaPhysicalSec =
    (currentPausedSimulationTimeUs - previousPausedSimulationTimeUs) / 1e6

activePhysicalTimeSec += deltaPhysicalSec
activeGameTimeSec += deltaPhysicalSec * previousLocalScale
```

The previous scale belongs to the interval ending at the current frame. Scale
changes therefore remain continuous and are not lifecycle boundaries.

Timer restart, timestamp regression, unavailable interval scale, invalid scale,
or an advancing pause-aware timestamp while explicitly paused emits a typed
clock diagnostic and a hard normalized boundary. No delta is guessed across
that interval. Pause events themselves add no time to either active clock.

## JSONL recording format

Every nonblank line is one independently versioned JSON object:

```json
{"recordVersion":1,"input":{"type":"source_connected","data":{"sourceEpoch":1}}}
{"recordVersion":1,"input":{"type":"frame","data":{"sourceEpoch":1,"sequence":1,"pausedSimulationTimeUs":0,"localScale":3.0,"gameTimeMinutes":100,"navigationDistanceM":null,"navigationTimeSec":null,"speedMps":0.0,"odometerKm":12.5}}}
{"recordVersion":1,"input":{"type":"paused"}}
```

Version 1 records only project-relevant fields. Parsing rejects unsupported
versions, malformed JSON, missing required fields, and schema/type errors with
line-numbered typed errors. Encoding and decoding are deterministic, and null
channel availability survives round trips. Because standard JSON has no NaN or
infinity representation, the encoder rejects non-finite numbers rather than
silently converting them to null.

## Replay

`replay_jsonl` performs this pipeline synchronously:

```text
JSONL parser -> TelemetryAdapter -> CalibrationEngine -> ReplayReport
```

`ReplayReport` contains ordered per-record adapter/core outputs, adapter
diagnostics, a compact summary, and the final estimator state. It never sleeps
or reads a clock. A developer can inspect a fixture with:

```bash
cargo run -p telemetry-adapter --bin replay -- \
  crates/telemetry-adapter/tests/fixtures/normal_drive.jsonl
```

The seven committed fixtures are synthetic normalized recordings. Reroute and
save/load magnitudes are derived from the live spike evidence, including the
observed approximately +45 km reroute and +788 m/+42.6 s load transient; they
are not raw personal game logs.

## Responsibility boundary

The adapter emits strong source/lifecycle boundaries, integrates clocks, and
rejects malformed raw values. The core detects route/odometer inconsistency.
The observed save/load transient is therefore caught by the core, while an
explicit `LoadOrRestart` input remains available when a future live adapter has
stronger evidence. Perfect implicit load classification is not claimed.
