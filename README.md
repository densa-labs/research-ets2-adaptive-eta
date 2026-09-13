# ETS2 Adaptive ETA — telemetry feasibility spike

This repository contains the completed disposable macOS telemetry probe and a
standalone, platform-neutral calibration-engine prototype. It does not yet
connect the two or implement the planned companion app.

The probe logs one rate-limited sample every two rendering seconds through
ETS2's SDK logger, plus immediate pause/start/timer and local-scale transition
records. In a standard Steam installation the resulting output is in:

```text
~/Library/Application Support/Euro Truck Simulator 2/game.log.txt
```

Build and verify the plugin:

```bash
rustup target add x86_64-apple-darwin
scripts/build-macos-spike.sh
```

Install it while ETS2 is fully closed:

```bash
scripts/install-macos-spike.sh
```

The installer writes only this filename and refuses to replace an existing
file:

```text
Euro Truck Simulator 2.app/Contents/MacOS/plugins/
libets2_adaptive_eta_telemetry_spike.dylib
```

Filter diagnostic records after a test session:

```bash
rg '\[adaptive-eta-spike\]' \
  "$HOME/Library/Application Support/Euro Truck Simulator 2/game.log.txt"
```

The exact dependency is `scs-sdk-plugin = 0.1.1`, locked by `Cargo.lock`. That
release was audited at upstream revision
`16a439a0892235051634fd4f54de3d8ab104f6d0` against the official SCS SDK 1.14
headers for every event, channel, value type, and flag used by this spike.

See `SPIKE_RESULTS.md` for the experiment protocol and current evidence.
See `CALIBRATION_ENGINE.md` for the focused core behavior and formulas.
