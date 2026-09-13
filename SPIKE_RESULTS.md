# Live telemetry feasibility spike

This document records evidence for the narrow macOS telemetry spike. It is not
product architecture documentation.

## Hypothesis under test

```text
scaled active game seconds
  = sum(delta paused_simulation_time seconds * interval local.scale)
```

The experiment must determine whether this duration is meaningfully comparable
to endpoint changes in `truck.navigation.time`.

## Automated evidence

The release plugin builds successfully for `x86_64-apple-darwin`. The installed
artifact is a thin x86-64 Mach-O dynamic library with the required
`_scs_telemetry_init` and `_scs_telemetry_shutdown` exports and a valid ad-hoc
signature. Unit tests, formatting, and Clippy checks pass. ETS2 1.60.1.7s loaded
the plugin and initialized all four requested events and all six requested
channels without a plugin-specific error.

## Live-test protocol

Each test begins after `navigationDistanceValid=true` and
`navigationTimeValid=true` appear. Preserve `game.log.txt` immediately after the
session because ETS2 rewrites it at the next process start.

### A. Normal continuous driving

Record the first and last sample across several kilometres. Compare:

```text
navigationTimeStart - navigationTimeEnd
integratedScaledActiveGameSecEnd - integratedScaledActiveGameSecStart
```

Also record odometer and navigation-distance changes. Equality is not assumed.

Status: **verified** on ETS2 1.60.1.7s at constant `localScale=3`.

The clean interval contained 232 logged samples with no paused or invalid
sample, timer boundary, scale transition, or reroute-like navigation increase:

| Measurement | Start | End | Change |
| --- | ---: | ---: | ---: |
| `pausedSimulationTimeUs` | 423,399,730 | 541,561,670 | +118.161940 s |
| `integratedScaledActiveGameSec` | 1,270.149192 | 1,624.635012 | +354.485820 s |
| `localScale` | 3 | 3 | unchanged |
| `navigationTimeSec` | 2,434.2280 | 1,674.4750 | -759.7530 s |
| `navigationDistanceM` | 29,029.406 | 24,369.523 | -4,659.883 m |
| `odometerKm` | 0.9432607 | 5.5720806 | +4.6288199 km |

For this interval, `actualConsumed / predictedConsumed` was
`354.485820 / 759.7530 = 0.466580349`. The unusually low single-interval value
is an observation, not a calibration recommendation. It is internally
consistent: the pause-aware interval times scale 3 equals the integrated game
duration; navigation time decreases only with route progress; and its endpoint
decrease is therefore dimensionally and behaviorally comparable to elapsed
game time. The exact route-distance decrease was 4.660 km rather than the
requested 5 km, but it is sufficient for this feasibility test's
several-kilometre requirement.

### B. Stationary and unpaused

Stop safely with navigation active and remain unpaused for at least 30 real
seconds. Determine whether navigation time remains fixed or counts down while
`integratedScaledActiveGameSec` advances.

Status: **verified** on ETS2 1.60.1.7s (telemetry API 1.1, schema 1.19).

A continuous 455-sample interval remained unpaused with absolute reported
speed below 0.01 m/s. Endpoint evidence:

| Measurement | Start | End | Change |
| --- | ---: | ---: | ---: |
| `pausedSimulationTimeUs` | 114,828,740 | 349,719,344 | +234.890604 s |
| `integratedScaledActiveGameSec` | 344.436222 | 1,049.108034 | +704.671812 s |
| `localScale` | 3 | 3 | unchanged |
| `navigationTimeSec` | 2,397.8127 | 2,397.8130 | -0.0003 s consumed |
| `navigationDistanceM` | 28,983.258 | 28,983.264 | -0.006 m progress |
| `odometerKm` | 0.895766900 | 0.895792540 | +0.000025640 km |
| `gameTimeMinutes` | 605 | 617 | +12 min |

The integrated result is exactly the measured pause-aware interval multiplied
by scale 3. Navigation time did **not** count down while game time advanced;
its 0.3 ms endpoint change and the 6 mm distance change are insignificant
telemetry/physics jitter. This supports interpreting a decrease in navigation
time as predicted route progress rather than elapsed game time. It does not by
itself validate the complete calibration ratio; normal-driving test A remains
required.

### C. Paused menu

Pause for at least 30 real seconds. Confirm that ordinary simulation/render
timestamps continue as observed, while `pausedSimulationTimeUs` and
`integratedScaledActiveGameSec` add effectively zero.

Status: **verified** after installing the 0.5 Hz diagnostic rebuild.

The SDK emitted explicit `paused` and `started` boundaries around a
70.713838-second pause. Boundary evidence:

| Measurement | Pause start | Resume | Change |
| --- | ---: | ---: | ---: |
| `simulationTimeUs` | 81,280,082 | 151,993,920 | +70.713838 s |
| `renderTimeUs` | 81,262,964 | 151,975,664 | +70.712700 s |
| `pausedSimulationTimeUs` | 50,814,634 | 50,814,634 | 0 s |
| `integratedScaledActiveGameSec` | 152.393904 | 152.393904 | 0 s |
| `navigationTimeSec` | 1,625.8932 | 1,625.8932 | 0 s |
| `navigationDistanceM` | 23,871.287 | 23,871.287 | 0 m |
| `odometerKm` | 6.14623 | 6.14623 | 0 km |
| `gameTimeMinutes` | 678 | 678 | 0 min |
| `localScale` | 3 | 3 | unchanged and valid |

On the first frame after resume, `pausedSimulationTimeUs` advanced by 16,666 us
and the integrated clock advanced by 0.049998 s, exactly matching scale 3.
Pauses therefore add effectively zero to the proposed active game clock without
requiring wall-clock subtraction.

The first repeat attempt also produced a separate spike finding: verbose 2 Hz
diagnostics exhausted ETS2's approximately 1 MiB `game.log.txt` limit in about
16 minutes. The probe now emits periodic records at 0.5 Hz while retaining
immediate lifecycle, timer, and scale-transition records.

### D. City/open-road scale transition

Capture samples on both sides of a `localScale` change. The plugin applies the
previously observed scale to each preceding frame interval and never applies a
final scale retroactively to the whole observation.

Status: **verified** across city scale 3 and open-road scale 19.

The clean test interval contained 45 logged observations, no paused or invalid
sample, and no timer restart. Endpoint evidence:

| Measurement | Start | End | Change |
| --- | ---: | ---: | ---: |
| `pausedSimulationTimeUs` | 148,360,732 | 226,857,592 | +78.496860 s |
| `integratedScaledActiveGameSec` | 445.032198 | 1,120.238522 | +675.206324 s |
| `navigationTimeSec` | 1,548.2633 | 764.82965 | -783.4337 s |
| `navigationDistanceM` | 23,784.955 | 10,027.112 | -13,757.843 m |
| `odometerKm` | 6.1654100 | 19.8295440 | +13.6641340 km |
| `gameTimeMinutes` | 683 | 695 | +12 min |

At the primary 3-to-19 transition, the preceding 0.549978-second interval added
1.649934 integrated seconds (`0.549978 * 3`). The following 1.999920-second
interval added 37.998480 seconds (`1.999920 * 19`). This confirms that the
plugin uses the scale applicable to each preceding interval and does not apply
the final scale retroactively.

Near a zone boundary while the truck slowed to rest, `local.scale` oscillated
3-to-19-to-3-to-19-to-3 across five transitions in roughly 0.26 real seconds.
Every logged integrated delta matched the prior interval's scale. Future core
logic must therefore integrate at frame granularity; it must not assume a scale
transition is singular or use sparse observation-point scale for a whole
sample. A scale change alone is not a discontinuity and need not invalidate a
sample when the integrated clock remains valid.

The interval's illustrative ratio was `675.206324 / 783.4337 = 0.861855093`.

### E. Reroute

Miss a turn with navigation active. Compare the one-step changes in navigation
distance/time with odometer progress and the two simulation-time deltas.

Status: **verified** for an intentional missed-turn recalculation.

The recalculation occurred within one 1.999920-second observation interval:

| Measurement | Before | After | Change |
| --- | ---: | ---: | ---: |
| `navigationDistanceM` | 705.526 | 45,788.004 | +45,082.478 m |
| `navigationTimeSec` | 179.4222 | 2,901.6357 | +2,722.2135 s |
| `odometerKm` | 29.323456 | 29.388058 | +0.064602 km |
| `integratedScaledActiveGameSec` | 2,210.878228 | 2,216.877988 | +5.999760 s |
| `localScale` | 3 | 3 | unchanged |

Both navigation channels remained valid, raw and integrated clocks remained
continuous, and no timer restart or explicit reroute event accompanied the
change. Subsequent samples resumed ordinary decreasing distance and ETA.

The SDK surface used by this spike therefore does not directly announce this
reroute, but the event is reliably inferable here: tens of kilometres of route
change cannot be explained by 64.6 m of odometer movement. Future sampling must
discard the open sample and reset its anchor on a significant positive
distance/ETA jump or, more generally, on navigation change that is physically
inconsistent with odometer progress. Positive-jump thresholds alone will not
catch every possible reroute because a recalculation can also shorten a route.

### F. Timer restart/load

If practical, load a save. Look for both:

```text
boundary kind=timerRestart
timerRestart=true
```

Confirm the raw timestamps reset and `clockEpoch` increments while the spike's
integrated clock restarts at zero.

Status: **verified with an unexpected result** on loading a newly created save.

The game log explicitly recorded `load_game 5 1` and `Loading save` while the
SDK continued delivering frames. The load was bracketed by the normal paused
and started events, but it did **not** set `timerRestart=true`, did not emit a
`boundary kind=timerRestart`, and did not increment `clockEpoch`:

| Measurement | Before load | First resumed frame | Behavior |
| --- | ---: | ---: | --- |
| `clockEpoch` | 2 | 2 | unchanged |
| `pausedSimulationTimeUs` | 656,023,758 | 656,040,424 | paused during load; then +0.016666 s |
| `simulationTimeUs` | 805,201,124 | 810,300,920 | monotonic |
| `renderTimeUs` | 805,176,851 | 810,278,902 | monotonic |
| `integratedScaledActiveGameSec` | 2,917.316636 | 2,917.366634 | +0.049998 s after resume |
| `navigationDistanceM` | 44,229.574 | 45,017.700 | transiently +788.126 m |
| `navigationTimeSec` | 2,780.7605 | 2,823.3972 | transiently +42.6367 s |

Two seconds later, both navigation values returned to their pre-load values.
All relevant channels stayed valid. This demonstrates that a same-session save
load can produce transient route values without resetting any SDK timestamp or
raising the timer-restart flag.

The timer-restart flag itself is confirmed functional at initial world startup:
the spike observed `timerRestart=true`, incremented `clockEpoch`, and received
zeroed/reset frame timestamps. It must remain a hard boundary whenever present,
but it is not a complete save-load detector in this ETS2 version. The future
sampler must also reset on lifecycle/load stabilization: discard its open sample
on a paused/started world transition and wait for navigation values to remain
stable before establishing a new anchor. No calibration data may span such a
transition.

## Calibration conclusion

**VALIDATED WITH CHANGES.** For an uninterrupted interval with valid navigation,
the endpoint decrease in `truck.navigation.time` behaves as predicted route
progress, not as an ordinary countdown: it remained fixed while the truck was
stationary even though scaled game time advanced. During normal driving and
across scale changes, the pause-aware, per-frame scaled clock is dimensionally
and behaviorally comparable to navigation-time progress.

The ratio is valid only inside a continuous, stabilized sampling epoch. An open
sample must be discarded across pause/start world transitions, timer restart,
timestamp regression, channel invalidity, or a reroute/load discontinuity. In
addition, a sample needs positive meaningful route progress; stationary time
cannot form a ratio because predicted consumed time is approximately zero.

## Milestone result

### Build and SDK

- Build: successful with Rust target `x86_64-apple-darwin`.
- Artifact: `target/x86_64-apple-darwin/release/libets2_adaptive_eta_telemetry_spike.dylib`.
- Format: thin 64-bit x86-64 Mach-O dynamic library.
- Signing: valid ad-hoc signature.
- Exports: `_scs_telemetry_init` and `_scs_telemetry_shutdown` only.
- Install location: `Euro Truck Simulator 2.app/Contents/MacOS/plugins/libets2_adaptive_eta_telemetry_spike.dylib`.
- Live SDK initialization: successful, with four events and six channels.
- Events: frame start, frame end, paused, and started.
- Channels: `local.scale`, `game.time`, `truck.speed`, `truck.odometer`,
  `truck.navigation.distance`, and `truck.navigation.time`.
- Validity: every subscribed channel preserves the SDK no-value state as an
  explicit `Option`; diagnostic output includes validity booleans.
- Timer handling: restart flag and backwards timestamps create hard clock
  epochs; unavailable interval scale also prevents integration across a gap.
- Plugin errors: none observed. Other errors in `game.log.txt` were emitted by
  unrelated game/profile content.

Automated verification completed with six passing unit tests, clean formatting,
clean Clippy with warnings denied, matching SHA-256 hashes for the built and
installed artifact, successful signature verification, and exact export
verification.

### Live validation matrix

| Test | Status |
| --- | --- |
| Normal continuous driving | **verified** |
| Stationary and unpaused | **verified** |
| Pause/menu | **verified** |
| City/open-road scale transition | **verified** |
| Intentional reroute | **verified** |
| Timer restart/save load | **verified** — restart flag at world startup; same-session save load did not raise it |

### Required design changes

1. Integrate `paused_simulation_time` at frame granularity using the scale from
   the preceding interval. Never apply a sparsely sampled or final scale to an
   entire observation.
2. Treat timer restart and timestamp regression as hard boundaries, but do not
   rely on timer restart to identify every save load.
3. Conservatively reset an open sample on paused/started world transitions and
   require stable valid navigation after resume before anchoring again. This
   catches the observed load behavior at the cost of also ending samples around
   ordinary menu pauses.
4. Detect reroutes through navigation changes that are physically inconsistent
   with odometer progress. Check both route increases and route-shortening
   discontinuities.
5. Require meaningful positive navigation-time progress before calculating a
   ratio. Stationary unpaused time is valid elapsed simulated time, but produces
   no predicted route progress and must not become a calibration sample by
   itself.
6. Keep SDK callback work bounded. The SDK logger is suitable only for this
   throttled spike; production transport must not depend on the size-limited
   game log.

### Smallest next milestone

Build a standalone, deterministic calibration-engine prototype with no live
transport or UI. Feed it recorded/synthetic observation sequences and implement
only sampling anchors, invalidation boundaries, the actual/predicted ratio,
outlier policy, smoothing, and tests. The live plugin-to-process transport
should follow only after those semantics are accepted.
