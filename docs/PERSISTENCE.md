# Persistent calibration profile

Adaptive ETA stores the minimum global estimator evidence needed to resume learning after the
companion runtime restarts. Persistence is owned by `adaptive-eta-runtime`; the SCS plugin,
transport, telemetry adapter, and calibration core never perform filesystem I/O.

## Durable and transient state

The core exposes a validated `CalibrationSnapshot` containing:

- `modelVersion`: identifies the calibration mathematics represented by the snapshot.
- `logFactor`: the canonical EWMA state used by the next accepted update.
- `evidenceDistanceKm`: the accepted distance used to calculate confidence.
- `sampleCount`: the accepted sample count used to calculate confidence.

Learned factor, confidence, display factor, and Adaptive ETA are derived and are not stored.
Session diagnostic counters do not affect future estimates and are not stored.

Every process starts with clean adapter, transport-continuity, timing, stabilization, lifecycle,
and sampling-window state. In particular, the profile contains no raw telemetry, route or trip
history, timestamps, sequence/epoch/ordinal values, navigation values, odometer, scale, pause/job
state, open sample, or current ETA. Restoring a profile therefore cannot imply continuity across a
process boundary, and a new sample still requires normal stabilization.

## Schema v1

The default ETS2 profile is one JSON object:

```json
{
  "schemaVersion": 1,
  "product": "adaptive-eta",
  "game": "ets2",
  "calibration": {
    "modelVersion": 1,
    "logFactor": 0.0,
    "evidenceDistanceKm": 0.0,
    "sampleCount": 0
  }
}
```

The schema uses platform-neutral JSON types and contains no paths or machine identifiers. JSON
floating-point serialization preserves the finite `f64` state needed for exact Rust round trips.
The current core validates model version, finiteness, model-supported factor range, nonnegative and
bounded evidence, sample-count bounds, and zero-evidence consistency before restoration.

An unsupported schema or model version, malformed/truncated JSON, identity mismatch, missing field,
or invalid numeric invariant is rejected. The runtime starts with a neutral in-memory estimator and
enters degraded read-only profile mode for that process, preserving the original file for diagnosis
instead of silently overwriting or downgrading it. A missing profile is the normal first-run state
and permits creation after the first accepted sample.

## Locations

The current-user application-data directory is resolved through the platform directory API:

- Windows: `%LOCALAPPDATA%\Adaptive ETA\profiles\ets2\default.json`
- Linux: `$XDG_DATA_HOME/adaptive-eta/profiles/ets2/default.json`, falling back to
  `~/.local/share/adaptive-eta/profiles/ets2/default.json`
- macOS: `~/Library/Application Support/Adaptive ETA/profiles/ets2/default.json`

Only path resolution differs by platform. The store, schema, validation, save trigger, and writer
are shared. On Unix, newly created directories and profile files request modes `0700` and `0600`;
existing permissions are not rewritten.

## Save and crash behavior

An accepted sample returns a new durable snapshot. The receive loop places it in one bounded,
coalescing writer slot; newer pending state replaces older pending state, one writer thread owns all
disk I/O, and no telemetry/plugin callback waits for disk. Writer diagnostics also use a bounded
channel. Failures are reported by typed stage and I/O kind and may be retried only when a later
accepted sample changes state.

Each save validates and serializes the snapshot, creates a unique temporary file beside the final
profile, writes the full object, flushes it with `sync_all`, and atomically replaces the final path.
Unix uses same-filesystem `rename` and then synchronizes the containing directory. Windows uses
`MoveFileExW` with replace-existing and write-through flags. If creation, writing, flushing, or
replacement fails, the previous final file remains untouched and the owned temporary file is
removed where possible. Stray temporary files from process interruption are ignored, so the prior
final profile remains authoritative. No backup or journal is used.

On clean shutdown, the runtime stops receiving telemetry and gives the worker five seconds to drain
the newest pending snapshot. The timeout cannot make crash safety depend on shutdown; every accepted
sample already requests a save. Read, write, disk-full, permission, and path-resolution failures do
not terminate the runtime or affect the valid in-memory estimator.

## Verification strategy

Tests use disposable directories. They cover first run, exact save/load and continued-learning
equivalence, clean transient state and stabilization after restart, Unix permissions, platform path
construction, interrupted temporary writes, malformed/truncated/missing fields, unsupported
versions, invalid numeric/evidence invariants, typed filesystem failures, coalescing, and bounded
writer shutdown. Automated tests never access the user's real profile.
