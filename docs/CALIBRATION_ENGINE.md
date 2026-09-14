# Calibration engine prototype

This document describes the standalone core prototype. It has no dependency on
the SCS SDK, operating system, filesystem, sockets, UI, or wall clock.

## Input and epochs

The future live adapter supplies normalized frames with two monotonic clocks:

- `active_game_time_sec` is integrated scaled game time and is the calibration
  numerator.
- `active_physical_time_sec` is unscaled, pause-aware simulation time and is
  used only for stabilization, telemetry-gap, and stationary/AFK rules.

Lifecycle events are explicit inputs. A valid sample never crosses a source
epoch, pause/start transition, disconnect, timer restart, job boundary,
ferry/train event, navigation-invalid period, explicit load, timestamp
regression, untrustworthy telemetry gap, inferred navigation discontinuity, or
long stationary boundary.

The default inference tolerances are a physical frame gap over 5 seconds, a
sequence gap over 120, a navigation-distance increase over 100 m, a
navigation-time increase over 30 seconds, or an odometer regression beyond
0.01 km. These conservative prototype constants remain configurable.

Following a boundary, navigation must remain valid and continuous for five
unscaled active seconds before a new anchor is established. A normal map time
scale change is absent from this model because the adapter has already
integrated it correctly; it is not a boundary.

## Sampling and decisions

A candidate window closes after both 8 km of odometer distance and four scaled
game minutes. It is accepted only with at least three minutes of predicted
navigation-time progress and meaningful positive finite deltas.

```text
distanceDrivenKm = odometerEnd - odometerStart
routeProgressKm = (navigationDistanceStart - navigationDistanceEnd) / 1000
predictedConsumedSec = navigationTimeStart - navigationTimeEnd
actualConsumedSec = activeGameTimeEnd - activeGameTimeStart
rawRatio = actualConsumedSec / predictedConsumedSec
```

Route progress must satisfy:

```text
abs(routeProgressKm - distanceDrivenKm)
    <= 0.75 km + 0.25 * distanceDrivenKm
```

Ratios outside `[0.50, 1.75]` are rejected. Ratios inside that hard range are
clamped to `[0.67, 1.50]`; diagnostics distinguish unchanged, bounded, invalid,
discontinuous, and hard-outlier outcomes.

Short unpaused stops remain part of elapsed game time. A continuous 120 seconds
below 0.5 m/s, measured with the unscaled active clock, discards the window.

## Estimator and confidence

For every accepted sample:

```text
weightKm = min(distanceDrivenKm, 12.5)
alpha = min(0.05, 1 - exp(-weightKm / 250))
logFactor = logFactor + alpha * (ln(boundedRatio) - logFactor)
learnedFactor = exp(logFactor)
```

Accepted sample count `N` and accepted distance `D` are evidence. Rejections do
not change them.

```text
distanceConfidence = D / (D + 200)
sampleConfidence = N / (N + 20)
confidence = min(distanceConfidence, sampleConfidence)
displayFactor = 1 + confidence * (learnedFactor - 1)
adaptiveEtaSec = gameEtaSec * displayFactor
```

The learned factor is the smoothed estimate. The displayed factor deliberately
stays closer to ETS2's factor of 1.0 until evidence accumulates. With no
evidence, adaptive ETA equals the supplied game ETA exactly.

## Save/load responsibility

The live +788 m/+42.6 s load transient is rejected by inferred navigation-jump
checks, and an explicit `LoadOrRestart` boundary is also supported. Inference is
not claimed to identify every possible load. The future adapter must emit that
boundary whenever it has stronger lifecycle evidence; the coordinator should
prefer discarding a window over learning across an ambiguous load.
