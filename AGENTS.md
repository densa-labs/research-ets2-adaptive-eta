# AGENTS.md — ETS2 Adaptive ETA

## Project identity

**Adaptive ETA** is a Densa Labs project for Euro Truck Simulator 2 (ETS2), with reasonable portability to American Truck Simulator (ATS) where SCS interfaces are shared.

Preferred permanent repository:

```text
densa-labs/ets2-adaptive-eta
```

Product branding:

```text
Adaptive ETA by Densa Labs
```

Development philosophy:

> **Get it working first, then make it clean/smart.**

Do not let advanced contextual ML, map parsing, speculative telemetry, or perfect distribution block the first complete release.

---

## Product goal

Adaptive ETA should beat ETS2's built-in Route Advisor ETA by learning how the individual player actually drives.

v0.1 is intentionally a **global personalized estimator**:

```text
predictedConsumedSec =
    navigationTimeStart - navigationTimeEnd

actualConsumedSec =
    integrated active scaled simulation time

rawRatio =
    actualConsumedSec / predictedConsumedSec
```

Repeated valid samples update a personalized correction factor:

```text
ETS2 ETA × personalized correction = Adaptive ETA
```

With no learned evidence, Adaptive ETA must equal stock ETS2 ETA exactly.

The long-term model is contextual, but the project should remain small, explainable, local-first, and measurable rather than turning into unnecessary deep learning.

---

## Version roadmap

### v0.1 — complete usable product

v0.1 must include the full core UX, not just the estimator:

```text
global personalized ETA
live ETS2 integration
persistent calibration/profile
real performance measurement
minimal Route Advisor-style Adaptive ETA display
installer/setup flow
post-install success flow
Adaptive ETA management app
settings
View Performance
diagnostics
repair
uninstall
keep/delete learned-data choice on uninstall
installation/update health handling
developer-only ETA evaluation tooling
real-route validation
Path B Workshop distribution work
```

v0.2 must not be “finish the app.” It should make prediction smarter.

### v0.2 — contextual personalized ETA

Candidate direct context already researched:

```text
truck.navigation.speed.limit
truck.world.placement
truck.local.acceleration.linear
truck.effective.brake
truck.effective.throttle
truck.cruise_control
cargo.mass
is.special.job
```

Useful derived context:

```text
speed / navigation speed limit
stop fraction
stop frequency
braking frequency
grade
curvature / heading change per km
spatial cell
direction bucket
segment historical factor
cargo-mass bucket
```

The v0.1 global estimator remains the **prior/fallback** when contextual evidence is sparse.

Prefer simple methods such as contextual EWMA, regularized online regression, hierarchical regression, or online gradient methods. Do not jump directly to neural networks.

### Optional v0.3 — map enrichment

Only pursue if measured accuracy gains justify the complexity.

Possible additions:

```text
road/speed class
freeway/local-road classification
city/slow-time context
intersection density
traffic-light existence/density
lane count
specific road geometry
road/prefab context
route-ahead enrichment where defensible
```

An independently reconstructed route must never be described as guaranteed to equal ETS2's actual Route Advisor route.

---

## Current implementation status

Completed:

1. Native live telemetry spike
2. Deterministic calibration core
3. Normalized telemetry adapter and deterministic record/replay harness

Relevant local commits reported by the user:

```text
46cd8a4 — verified spike/core baseline
26538c7 — normalized adapter and deterministic replay milestone
```

At record/replay completion:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

all passed.

Reported results:

```text
74 workspace tests
32 adapter/record/replay tests
7 replay fixtures
0 failures
0 warnings
```

Working tree was clean. No remote was configured and nothing was pushed.

Do not assume later milestones are complete unless the user explicitly reports completion.

---

## Validated telemetry behavior

Verified telemetry includes:

```text
local.scale
game.time
truck.speed
truck.odometer
truck.navigation.distance
truck.navigation.time
```

Verified SDK lifecycle/events include:

```text
frame_start
frame_end
paused
started
configuration
gameplay events
```

Within a stable epoch:

```text
actualConsumed =
    Σ(delta paused_simulation_time × interval local.scale)

predictedConsumed =
    navigationTimeStart - navigationTimeEnd

sampleRatio =
    actualConsumed / predictedConsumed
```

Important empirical finding:

`truck.navigation.time` behaves like an estimated remaining route duration, not a simple countdown. While stationary and unpaused, scaled game time can advance while navigation time remains essentially unchanged.

Therefore endpoint navigation-time difference is useful as the amount of ETS2-predicted travel time consumed.

---

## Timing rules

Use `paused_simulation_time` as the primary pause-aware interval clock.

Integrate `local.scale` **per interval**.

Normal scale changes are **not lifecycle boundaries**.

The completed adapter uses:

```text
deltaPhysicalSec =
    delta(pausedSimulationTimeUs) / 1,000,000

activePhysicalTimeSec += deltaPhysicalSec

activeGameTimeSec +=
    deltaPhysicalSec × previousLocalScale
```

The previous scale owns the interval ending at the current observation.

Do not guess timing across:

```text
timer restart
timestamp regression
missing interval scale
invalid interval scale
unexpected pause-aware timestamp advancement while explicitly paused
source restart
```

`game.time` is preserved but currently has no mathematical role in calibration.

---

## Save/load and lifecycle findings

Do not rely only on `timerRestart`.

An observed same-session save/load did not reliably emit it.

Observed transient was approximately:

```text
navigation distance: +788 m
navigation time:     +42.6 sec
```

before returning near prior values.

Samples must not cross unstable load/world transitions.

Short ordinary stops are legitimate travel behavior and should remain part of calibration.

---

## Deterministic calibration core

Current sampling parameters:

```text
Target distance                 8 km
Minimum active game time        240 sec
Minimum predicted consumption   180 sec
Stabilization                    5 unscaled active sec
Stationary threshold             abs(speed) < 0.5 m/s
Long-stationary cutoff           120 unscaled active sec
```

Sample definitions:

```text
distanceDrivenKm =
    odometerEnd - odometerStart

routeProgressKm =
    (navigationDistanceStart - navigationDistanceEnd) / 1000

predictedConsumedSec =
    navigationTimeStart - navigationTimeEnd

actualConsumedSec =
    activeGameTimeEnd - activeGameTimeStart

rawRatio =
    actualConsumedSec / predictedConsumedSec
```

Consistency requirement:

```text
abs(routeProgressKm - distanceDrivenKm)
    <= 0.75 + 0.25 × distanceDrivenKm
```

Outlier policy:

```text
hard acceptance range: 0.50–1.75
applied bounded range: 0.67–1.50
```

Estimator:

```text
weightKm = min(distanceDrivenKm, 12.5)

alpha =
    min(0.05, 1 - exp(-weightKm / 250))

logFactor =
    logFactor
    + alpha × (ln(boundedRatio) - logFactor)

learnedFactor = exp(logFactor)
```

Confidence:

```text
distanceConfidence = D / (D + 200)
sampleConfidence   = N / (N + 20)

confidence =
    min(distanceConfidence, sampleConfidence)

displayFactor =
    1 + confidence × (learnedFactor - 1)

adaptiveEtaSec =
    gameEtaSec × displayFactor
```

Do not casually retune these constants without real driving evidence.

---

## Calibration boundaries / data quality

The system already protects against cases including:

```text
pause/world-start transitions
disconnect
source epoch changes
timer restart
job change/end
ferry/train
explicit load/restart
invalid navigation
invalid/regressing timestamps
regressing telemetry sequence
large sequence gaps
large frame gaps
navigation-distance increases
navigation-time increases
odometer regression
route progress inconsistent with odometer
long stationary/AFK periods
```

The core also recognizes adapter-facing boundaries including:

```text
TimingDiscontinuity
AdapterRejectedInput
```

These are lifecycle/data-quality rules, not contextual ML features.

---

## Telemetry adapter and replay contract

The completed `telemetry-adapter` crate includes:

```text
raw.rs
clock.rs
adapter.rs
record.rs
replay.rs
src/bin/replay.rs
TELEMETRY_ADAPTER.md
```

Input is `RawInput`, containing lifecycle events or `RawFrame` values including source epoch/sequence, pause-aware timestamps, scale, game time, navigation values with explicit availability, speed, odometer, timer restart, and lifecycle/job/ferry/train/source events.

Output is an ordered list of:

```text
adaptive_eta_core::EngineInput
AdapterDiagnostic
```

The adapter must not mutate estimator state.

Source epoch and sequence remain visible to the core.

Unavailable values remain unavailable/null; never fabricate zero.

Recording format is versioned JSONL with current schema:

```text
recordVersion = 1
```

Replay is deterministic and performs no sleeping or wall-clock reads:

```text
JSONL
→ parser
→ telemetry adapter
→ calibration core
→ ReplayReport
```

Regression fixtures cover:

```text
normal driving
pause
reroute
save/load transient
scale transition
navigation invalidity
source restart
```

Repeated replay of the same capture must produce identical normalized/core decisions and final state.

---

## NEXT MILESTONE — live adapter-integration spike

Unless the user changes priorities, this is the next engineering task.

Use the existing diagnostic SCS telemetry plugin to translate actual SCS callback ordering and live values into `RawInput`.

The core acceptance test is:

```text
LIVE

SCS callbacks
→ RawInput
→ adapter
→ calibration core
→ decisions A

and simultaneously:
→ replay-compatible capture

OFFLINE

same capture
→ parser
→ adapter
→ calibration core
→ decisions B

REQUIRE:
A == B
```

Exercise real behavior around:

```text
normal driving
pause/resume
local.scale transitions
reroute
save/load where practical
navigation invalidity
source/lifecycle transitions
```

Observe remaining risks:

```text
actual callback ordering into RawInput
transient disappearance/invalidity of scale
pause-event ordering relative to the last active frame
same-session save/load classification
```

### Explicit non-goals for this milestone

Do **not** add yet:

```text
production sockets/IPC
persistent player profiles
production persistence
management dashboard
Route Advisor HUD/overlay
contextual ML
static map parsing
```

Do not automatically begin the following milestone.

---

## v0.1 management app and UX

The setup app should evolve into the persistent **Adaptive ETA** management app.

After installation, conceptually:

```text
Adaptive ETA

Status
✓ Installed
✓ ETS2 plugin detected
✓ Overlay/helper healthy
✓ Calibration healthy

[ View Performance ]
[ Settings ]
[ Diagnostics ]
[ Repair ]
[ Uninstall Adaptive ETA ]
```

Post-install messaging should make clear that the app remains useful:

```text
Adaptive ETA is installed and ready to use.

Launch Euro Truck Simulator 2 normally and Adaptive ETA will start automatically.

For settings, performance, diagnostics, repair, and uninstall options,
open the Adaptive ETA app at any time.
```

### Performance

User-facing performance should prove whether Adaptive ETA helps.

Useful metrics:

```text
ETS2 MAE
Adaptive ETA MAE
percentage improvement
distance learned
sample count
confidence
current factor
```

Never present hypothetical numbers as real measurements.

### Diagnostics

Diagnostics should be understandable by ordinary users.

Useful checks/actions:

```text
ETS2 installation found
plugin installed
plugin version
Workshop payload found
helper/overlay running
telemetry connected
navigation data valid
calibration database healthy
last telemetry timestamp

Run Diagnostics
Repair Installation
Export Diagnostic Report
Open Logs
```

Diagnostic exports should avoid unnecessary private information.

### Uninstall

Provide explicit uninstall support for native/runtime files and let the user choose whether to keep or delete learned profile data.

Workshop unsubscribe alone cannot be assumed to clean copied native components.

---

## Developer-only ETA evaluation

v0.1 should include a private developer evaluation mode comparing stock ETS2 ETA and Adaptive ETA.

At a checkpoint, freeze both predictions:

```text
gamePredictionSec     = current navigation ETA
adaptivePredictionSec = current Adaptive ETA
```

At destination, compute actual scaled active duration from checkpoint to arrival and score:

```text
gameError =
    abs(gamePredictionSec - actualRemainingSec)

adaptiveError =
    abs(adaptivePredictionSec - actualRemainingSec)
```

Historical predictions must remain immutable even if the model learns more later.

Reuse lifecycle logic to exclude contaminated checkpoints such as reroutes, ferry/train, load/restart, invalid navigation, source restart, or job changes.

The main validation metric is real-world ETA accuracy versus stock ETS2, especially MAE and median absolute error.

Developer tooling may be verbose and should preferably be compile-time gated so normal builds do not expose it.

---

## Steam Workshop distribution target — Path B

Preferred target:

> **One Steam Workshop subscription contains/delivers everything needed, with one explicit setup/activation step.**

Desired flow:

```text
Steam Workshop item
├── ETS2 UI integration
├── native Adaptive ETA plugin/engine
├── overlay/helper
└── setup payload

Subscribe
→ Steam downloads everything
→ user runs Adaptive ETA Setup once
→ setup installs/activates native components
→ future sessions launch ETS2 normally
```

The Workshop item should ideally be the sole download/CDN source.

Fallback is **Path C**, a Workshop-centered design with a very small external bootstrap, only if Steam/SCS constraints prevent Path B.

Do not claim Path B is proven until SCS Workshop Uploader behavior and policy are validated.

Technical uploader acceptance is not the same as public policy approval.

---

## macOS packaging rules

Do not assume Apple Developer Program membership.

The project must remain buildable/developable without paid Developer ID signing/notarization. Notarization may be optional later polish.

Do not implement or recommend:

```text
Gatekeeper bypasses
security-control bypasses
DLL/dylib injection
memory patching
Steam client exploitation
path traversal
privilege escalation
modifying Steam or ETS2 binaries
deceptive automatic execution
```

Conceptual locations:

```text
Adaptive ETA.app
~/Library/Application Support/Adaptive ETA/
ETS2.app/Contents/MacOS/plugins/AdaptiveETA.dylib
Steam Workshop folder as source/update payload
```

The management app should support Repair if ETS2 updates invalidate/remove installed runtime components.

---

## Route Advisor UX

Keep the in-game experience minimal.

Target concept:

```text
ETS2 telemetry
→ Adaptive ETA engine
→ personalized ETA
→ Workshop UI hides/repositions stock ETA
→ overlay/helper displays Adaptive ETA
```

While driving, users mainly need the Adaptive ETA value.

Do not put confidence graphs, internal factors, sample counts, diagnostics, or developer statistics into the driving HUD.

Detailed information belongs in the management app.

---

## Contextual research constraints

The public SCS interfaces do **not** directly provide:

```text
live AI vehicle positions
nearby AI vehicle count
actual live traffic density
queue length
traffic-jam state
current traffic-light state
direct current rain intensity
wet-road state
fog/visibility
true runtime weather state
exact Route Advisor route polyline
route edge sequence
next maneuver
current canonical road ID/name
upcoming speed-limit sequence
active random event ahead
active dynamic detour ahead
```

Do not invent these APIs.

### Traffic

`g_traffic` is a global traffic-intensity/configuration setting, not live congestion.

Traffic effects may be represented only by honest behavioral/impedance proxies such as speed relative to the limit, stopping, braking, slow fraction, variance, and route-progress rate.

### Weather

True current weather was not found in the researched public telemetry API.

Wipers/headlights/configuration are proxies only.

Never assume:

```text
wipers on == raining
```

### Geographic learning

`truck.world.placement` supports geographic personalization without map parsing.

A useful future segment key is:

```text
spatial cell
+ heading/direction bucket
+ optional elevation
+ optional speed-limit bucket
```

### Grade and curvature

These can be derived from placement history:

```text
grade ≈ Δelevation / horizontal_distance
curvature ≈ absolute heading change / distance
```

---

## Repository and documentation rules

When a permanent remote is created, prefer:

```text
densa-labs/ets2-adaptive-eta
```

Do not create the permanent project under a personal GitHub account unless explicitly instructed.

Local Git commits are allowed.

A missing remote is **not a blocker**.

Do not invent a remote and do not push unless explicitly authorized and a real remote exists.

Do not automatically rewrite `README.md`.

`README.md` is public-facing product documentation and should remain concise, approachable, and non-technical.

Only modify README when the current milestone explicitly owns a public-documentation change or the user explicitly asks.

Put architecture, internal contracts, experiments, and implementation detail into focused docs instead.

---

## Engineering workflow

Before substantial changes:

1. Inspect the actual repository state and relevant existing code/docs.
2. Treat validated behavior/tests as stronger evidence than assumptions.
3. Keep the current milestone narrowly scoped.
4. Preserve deterministic behavior where it already exists.
5. Add/update tests for changed behavior.
6. Run appropriate formatting, tests, and linting before claiming completion.
7. Report changed files, concrete findings, verification results, remaining risks, and Git status.
8. Do not automatically begin the next milestone.

Do **not** use Superpowers workflows for this project.

Do not add process bureaucracy merely for its own sake.

---

## Agent / prompt preferences

Preferred model for normal project implementation work:

```text
GPT-5.6 Sol
High reasoning
```

Use stronger/more expensive reasoning only when genuinely warranted or explicitly requested.

Prompts should be detailed, self-contained, evidence-driven, milestone-scoped, and explicit about non-goals, acceptance criteria, and verification.

Do not block work because the agent cannot inspect hidden model metadata.

---

## Research standard

For current SCS/Steam/platform questions, prefer:

```text
current official SCS SDK headers
official SCS documentation
official SCS modding documentation/tools
official Steam documentation
maintained implementations as secondary evidence
```

Clearly distinguish:

```text
confirmed direct capability
confirmed static-data capability
derived capability
heuristic/proxy
unsupported/internal state
unavailable state
```

Do not invent APIs.

For time-sensitive SDK versions, policies, Workshop rules, platform behavior, or distribution constraints, verify current sources rather than relying on stale recollection.

---

## North-star metric

Adaptive ETA succeeds if it produces **lower real-world ETA error than ETS2's stock ETA for the individual player**.

Measure:

```text
ETS2 stock ETA
vs v0.1 global personalized ETA
vs v0.2 contextual personalized ETA
vs optional v0.3 map-enriched ETA
```

A more complex model or feature should not be kept merely because it is technically interesting.

If it does not materially improve real ETA accuracy, simplify or remove it.
