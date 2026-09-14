**# AGENTS.md — ETS2 Adaptive ETA**

**## Project identity**

**\*\*Adaptive ETA\*\*** is a Densa Labs project for Euro Truck Simulator 2 (ETS2), with reasonable portability to American Truck Simulator (ATS) where SCS interfaces are shared.

Adaptive ETA is cross-platform by default. Windows, Linux, and macOS are first-class targets. macOS is the current development/validation environment, not the architectural default.

Preferred permanent repository:

\`\`\`text

densa-labs/ets2-adaptive-eta

\`\`\`

Product branding:

\`\`\`text

Adaptive ETA by Densa Labs

\`\`\`

Development philosophy:

\> **\*\*Get it working first, then make it clean/smart.\*\***

Do not let advanced contextual ML, map parsing, speculative telemetry, or perfect distribution block the first complete release.

**---**

**## Product goal**

Adaptive ETA should beat ETS2's built-in Route Advisor ETA by learning how the individual player actually drives.

v0.1 is intentionally a **\*\*global personalized estimator\*\***:

\`\`\`text

predictedConsumedSec =

    navigationTimeStart - navigationTimeEnd

actualConsumedSec =

    integrated active scaled simulation time

rawRatio =

    actualConsumedSec / predictedConsumedSec

\`\`\`

Repeated valid samples update a personalized correction factor:

\`\`\`text

ETS2 ETA × personalized correction = Adaptive ETA

\`\`\`

With no learned evidence, Adaptive ETA must equal stock ETS2 ETA exactly.

The long-term model is contextual, but the project should remain small, explainable, local-first, and measurable rather than turning into unnecessary deep learning.

**---**

**## Version roadmap**

**### v0.1 — complete usable product**

v0.1 must include the full core UX, not just the estimator:

\`\`\`text

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

\`\`\`

v0.2 must not be “finish the app.” It should make prediction smarter.

**### v0.2 — contextual personalized ETA**

Candidate direct context already researched:

\`\`\`text

truck.navigation.speed.limit

truck.world.placement

truck.local.acceleration.linear

truck.effective.brake

truck.effective.throttle

truck.cruise\_control

cargo.mass

is.special.job

\`\`\`

Useful derived context:

\`\`\`text

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

\`\`\`

The v0.1 global estimator remains the **\*\*prior/fallback\*\*** when contextual evidence is sparse.

Prefer simple methods such as contextual EWMA, regularized online regression, hierarchical regression, or online gradient methods. Do not jump directly to neural networks.

**### Optional v0.3 — map enrichment**

Only pursue if measured accuracy gains justify the complexity.

Possible additions:

\`\`\`text

road/speed class

freeway/local-road classification

city/slow-time context

intersection density

traffic-light existence/density

lane count

specific road geometry

road/prefab context

route-ahead enrichment where defensible

\`\`\`

An independently reconstructed route must never be described as guaranteed to equal ETS2's actual Route Advisor route.

**---**

**## Current implementation status**

Completed:

1\. Native live telemetry spike

2\. Deterministic calibration core

3\. Normalized telemetry adapter and deterministic record/replay harness

4\. Live SCS callback integration with exact live/replay parity

Relevant local commits reported by the user:

\`\`\`text

46cd8a4 — verified spike/core baseline

26538c7 — normalized adapter and deterministic replay milestone

2e79743 — live telemetry/replay parity implementation

9967574 — live-discovered parity and paused-frame fixes

20b76f1 — final live-validation evidence

\`\`\`

The live adapter milestone is fully live-validated.

Reported parity:

\`\`\`text

full driving session:      MATCH records=27759
corrected follow-up:       MATCH records=7812

\`\`\`

Equality remained exact. No epsilon or rounding tolerance was required.

An initial one-ULP mismatch was traced to JSON float parsing and fixed with exact `float_roundtrip` parsing, with regression coverage.

Live evidence includes:

\`\`\`text

7,801 complete RawFrame inputs
2,369 startup/paused frame pairs correctly suppressed
0 empty raw frames
0 callback-order anomalies
0 bridge diagnostics
0 adapter diagnostics
0 capture failures

\`\`\`

Observed active-frame callback order:

\`\`\`text

frame_start
local.scale
game.time
truck.speed
truck.odometer
truck.navigation.distance
truck.navigation.time
frame_end

\`\`\`

Also validated live:

\`\`\`text

pause/resume ordering
10 local.scale transitions between 3 and 19
no missing scale on observed active frames
navigation distance/time paired availability
reroute discontinuity handling
same-session save/load bounding without timerRestart
clean source shutdown
accepted calibration samples in real driving

\`\`\`

At final live validation:

\`\`\`text

79 tests passed
0 failures
0 warnings
cargo fmt --check passed
strict Clippy passed
x86-64 Mach-O plugin verified
valid ad-hoc signature
required SCS exports verified
installed plugin hash matched the verified build

\`\`\`

Do not assume a later milestone is complete unless the user explicitly reports completion.

**---**

**## Validated telemetry behavior**

Verified telemetry includes:

\`\`\`text

local.scale

game.time

truck.speed

truck.odometer

truck.navigation.distance

truck.navigation.time

\`\`\`

Verified SDK lifecycle/events include:

\`\`\`text

frame\_start

frame\_end

paused

started

configuration

gameplay events

\`\`\`

Within a stable epoch:

\`\`\`text

actualConsumed =

    Σ(delta paused\_simulation\_time × interval local.scale)

predictedConsumed =

    navigationTimeStart - navigationTimeEnd

sampleRatio =

    actualConsumed / predictedConsumed

\`\`\`

Important empirical finding:

\`truck.navigation.time\` behaves like an estimated remaining route duration, not a simple countdown. While stationary and unpaused, scaled game time can advance while navigation time remains essentially unchanged.

Therefore endpoint navigation-time difference is useful as the amount of ETS2-predicted travel time consumed.

**---**

**## Timing rules**

Use \`paused\_simulation\_time\` as the primary pause-aware interval clock.

Integrate \`local.scale\` **\*\*per interval\*\***.

Normal scale changes are **\*\*not lifecycle boundaries\*\***.

The completed adapter uses:

\`\`\`text

deltaPhysicalSec =

    delta(pausedSimulationTimeUs) / 1,000,000

activePhysicalTimeSec += deltaPhysicalSec

activeGameTimeSec +=

    deltaPhysicalSec × previousLocalScale

\`\`\`

The previous scale owns the interval ending at the current observation.

Do not guess timing across:

\`\`\`text

timer restart

timestamp regression

missing interval scale

invalid interval scale

unexpected pause-aware timestamp advancement while explicitly paused

source restart

\`\`\`

\`game.time\` is preserved but currently has no mathematical role in calibration.

**---**

**## Save/load and lifecycle findings**

Do not rely only on \`timerRestart\`.

An observed same-session save/load did not reliably emit it.

Observed transient was approximately:

\`\`\`text

navigation distance: +788 m

navigation time:     +42.6 sec

\`\`\`

before returning near prior values.

Samples must not cross unstable load/world transitions.

Short ordinary stops are legitimate travel behavior and should remain part of calibration.

**---**

**## Deterministic calibration core**

Current sampling parameters:

\`\`\`text

Target distance                 8 km

Minimum active game time        240 sec

Minimum predicted consumption   180 sec

Stabilization                    5 unscaled active sec

Stationary threshold             abs(speed) < 0.5 m/s

Long-stationary cutoff           120 unscaled active sec

\`\`\`

Sample definitions:

\`\`\`text

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

\`\`\`

Consistency requirement:

\`\`\`text

abs(routeProgressKm - distanceDrivenKm)

    <= 0.75 + 0.25 × distanceDrivenKm

\`\`\`

Outlier policy:

\`\`\`text

hard acceptance range: 0.50–1.75

applied bounded range: 0.67–1.50

\`\`\`

Estimator:

\`\`\`text

weightKm = min(distanceDrivenKm, 12.5)

alpha =

    min(0.05, 1 - exp(-weightKm / 250))

logFactor =

    logFactor

    \+ alpha × (ln(boundedRatio) - logFactor)

learnedFactor = exp(logFactor)

\`\`\`

Confidence:

\`\`\`text

distanceConfidence = D / (D + 200)

sampleConfidence   = N / (N + 20)

confidence =

    min(distanceConfidence, sampleConfidence)

displayFactor =

    1 + confidence × (learnedFactor - 1)

adaptiveEtaSec =

    gameEtaSec × displayFactor

\`\`\`

Do not casually retune these constants without real driving evidence.

**---**

**## Calibration boundaries / data quality**

The system already protects against cases including:

\`\`\`text

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

\`\`\`

The core also recognizes adapter-facing boundaries including:

\`\`\`text

TimingDiscontinuity

AdapterRejectedInput

\`\`\`

These are lifecycle/data-quality rules, not contextual ML features.

**---**

**## Telemetry adapter and replay contract**

The completed \`telemetry-adapter\` crate includes:

\`\`\`text

raw\.rs

clock.rs

adapter.rs

record.rs

replay.rs

src/bin/replay.rs

TELEMETRY\_ADAPTER.md

\`\`\`

Input is \`RawInput\`, containing lifecycle events or \`RawFrame\` values including source epoch/sequence, pause-aware timestamps, scale, game time, navigation values with explicit availability, speed, odometer, timer restart, and lifecycle/job/ferry/train/source events.

Output is an ordered list of:

\`\`\`text

adaptive\_eta\_core::EngineInput

AdapterDiagnostic

\`\`\`

The adapter must not mutate estimator state.

Source epoch and sequence remain visible to the core.

Unavailable values remain unavailable/null; never fabricate zero.

Recording format is versioned JSONL with current schema:

\`\`\`text

recordVersion = 1

\`\`\`

Replay is deterministic and performs no sleeping or wall-clock reads:

\`\`\`text

JSONL

→ parser

→ telemetry adapter

→ calibration core

→ ReplayReport

\`\`\`

Regression fixtures cover:

\`\`\`text

normal driving

pause

reroute

save/load transient

scale transition

navigation invalidity

source restart

\`\`\`

Repeated replay of the same capture must produce identical normalized/core decisions and final state.

**---**

**## NEXT MILESTONE — cross-platform architecture remediation**

Unless the user changes priorities, this is the next engineering task.

Adaptive ETA is now **cross-platform by default**.

Treat these as first-class targets:

\`\`\`text

Windows
Linux
macOS

\`\`\`

macOS is the current development/validation environment, not the architectural default.

The purpose of this milestone is **not** to port or rewrite the validated estimator. It is to audit the repository for accidental macOS-only assumptions before production transport, persistence, packaging, management UI, overlays, and installers make those assumptions expensive.

Preserve the validated platform-neutral pieces:

\`\`\`text

CalibrationCore
TelemetryAdapter
RawInput
record/replay
exact live/replay parity behavior
SCS callback → RawInput semantics
calibration mathematics

\`\`\`

Audit/remediate where necessary:

\`\`\`text

filesystem paths
runtime-directory handling
plugin build assumptions
plugin filename/extension assumptions
platform-specific dependencies
IPC assumptions
process launch assumptions
diagnostics path formatting
installer/package assumptions
OS-specific code leaking into shared crates
hard-coded macOS target triples or app-bundle paths

\`\`\`

The desired boundary is:

\`\`\`text

shared Rust
├── calibration core
├── telemetry adapter
├── transport protocol
├── runtime logic
├── persistence/profile format
└── evaluation logic

platform integration
├── Windows
│   ├── SCS plugin DLL
│   ├── Windows-local IPC
│   ├── Windows paths
│   ├── native Windows management UI
│   └── Windows installer/package lifecycle
├── Linux
│   ├── SCS plugin .so
│   ├── Unix-local IPC
│   ├── XDG paths
│   ├── native Linux management UI
│   └── Linux packaging lifecycle
└── macOS
    ├── SCS plugin .dylib
    ├── Unix-local IPC
    ├── macOS paths
    ├── SwiftUI/AppKit management UI
    └── macOS packaging lifecycle

\`\`\`

Use narrow platform modules/traits and `cfg(target_os = ...)` only where OS-specific behavior is genuinely required.

Do not rewrite working platform-neutral code merely to create abstractions.

Do not require Windows/Linux live validation during remediation if those environments are not available. The milestone should make the architecture ready for those targets and add compile/test coverage where practical.

After remediation, the next production milestone is:

\`\`\`text

production live transport + companion runtime

\`\`\`

That transport must be cross-platform by architecture:

\`\`\`text

shared protocol + shared continuity semantics

macOS/Linux backend:
    Unix-domain IPC or another justified Unix-local primitive

Windows backend:
    Windows-local IPC such as named pipes or another justified native primitive

\`\`\`

Do not define “Unix socket” as the product architecture itself.

**### Explicit non-goals for remediation**

Do **\*\*not\*\*** add yet:

\`\`\`text

production persistence/profiles
management app implementation
Route Advisor HUD/overlay
installer implementation
Workshop packaging implementation
contextual ML
static map parsing

\`\`\`

Do not automatically begin production transport after the remediation milestone.

**---**

**## v0.1 management app and UX**

The setup app should evolve into the persistent **\*\*Adaptive ETA\*\*** management app.

Management UI is native per platform rather than a shared Electron/web shell:

\`\`\`text

Windows → native Windows UI, preferably WinUI 3 or another justified native Windows desktop stack
Linux   → native Linux UI, preferably GTK or another justified native Linux stack
macOS   → SwiftUI + AppKit where needed

\`\`\`

Do not use Electron, an embedded browser shell, or a web-first desktop architecture merely to share frontend code.

Shared product behavior, runtime state, profile schema, diagnostics semantics, and performance metrics should remain common below the UI layer.

After installation, conceptually:

\`\`\`text

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

\`\`\`

Post-install messaging should make clear that the app remains useful:

\`\`\`text

Adaptive ETA is installed and ready to use.

Launch Euro Truck Simulator 2 normally and Adaptive ETA will start automatically.

For settings, performance, diagnostics, repair, and uninstall options,

open the Adaptive ETA app at any time.

\`\`\`

**### Performance**

User-facing performance should prove whether Adaptive ETA helps.

Useful metrics:

\`\`\`text

ETS2 MAE

Adaptive ETA MAE

percentage improvement

distance learned

sample count

confidence

current factor

\`\`\`

Never present hypothetical numbers as real measurements.

**### Diagnostics**

Diagnostics should be understandable by ordinary users.

Useful checks/actions:

\`\`\`text

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

\`\`\`

Diagnostic exports should avoid unnecessary private information.

**### Uninstall**

Provide explicit uninstall support for native/runtime files and let the user choose whether to keep or delete learned profile data.

Workshop unsubscribe alone cannot be assumed to clean copied native components.

**---**

**## Developer-only ETA evaluation**

v0.1 should include a private developer evaluation mode comparing stock ETS2 ETA and Adaptive ETA.

At a checkpoint, freeze both predictions:

\`\`\`text

gamePredictionSec     = current navigation ETA

adaptivePredictionSec = current Adaptive ETA

\`\`\`

At destination, compute actual scaled active duration from checkpoint to arrival and score:

\`\`\`text

gameError =

    abs(gamePredictionSec - actualRemainingSec)

adaptiveError =

    abs(adaptivePredictionSec - actualRemainingSec)

\`\`\`

Historical predictions must remain immutable even if the model learns more later.

Reuse lifecycle logic to exclude contaminated checkpoints such as reroutes, ferry/train, load/restart, invalid navigation, source restart, or job changes.

The main validation metric is real-world ETA accuracy versus stock ETS2, especially MAE and median absolute error.

Developer tooling may be verbose and should preferably be compile-time gated so normal builds do not expose it.

**---**

**## Steam Workshop distribution target — Path B**

Preferred target:

\> **\*\*One Steam Workshop subscription contains/delivers everything needed, with one explicit setup/activation step.\*\***

Desired flow:

\`\`\`text

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

\`\`\`

The Workshop item should ideally be the sole download/CDN source.

Fallback is **\*\*Path C\*\***, a Workshop-centered design with a very small external bootstrap, only if Steam/SCS constraints prevent Path B.

Do not claim Path B is proven until SCS Workshop Uploader behavior and policy are validated.

Technical uploader acceptance is not the same as public policy approval.

Path B packaging must account for platform-specific payloads rather than treating the macOS build as universal.

Conceptually the Workshop/source payload may contain platform-targeted components such as:

\`\`\`text

Windows:
    AdaptiveETA.dll
    Windows runtime/app
    Windows setup payload

Linux:
    libadaptive_eta.so
    Linux runtime/app
    Linux setup payload

macOS:
    AdaptiveETA.dylib
    Adaptive ETA.app/runtime
    macOS setup payload

shared:
    Workshop UI integration
    protocol/schema metadata
    licenses/notices
    version metadata

\`\`\`

The exact Workshop packaging mechanism remains subject to SCS uploader/policy validation.

**---**

**## Cross-platform packaging and platform-integration rules**

Adaptive ETA is one cross-platform product with platform-specific packaging shells.

Do not let installer, filesystem, signing, or UI requirements leak into shared calibration/runtime logic.

Shared artifacts/concepts should remain portable:

\`\`\`text

RawInput semantics
transport protocol schema
CalibrationCore
TelemetryAdapter
profile/persistence schema
performance/evaluation schema
diagnostics model
version numbers
update compatibility rules

\`\`\`

Platform-specific code should own only the parts that genuinely differ.

### Windows packaging rules

Windows is a first-class release target and should be treated as the largest likely ETS2 user platform.

Expected SCS plugin shape:

\`\`\`text

AdaptiveETA.dll

\`\`\`

Expected ETS2 plugin location:

\`\`\`text

<ETS2 install>\\bin\\win_x64\\plugins\\AdaptiveETA.dll

\`\`\`

ATS should use the corresponding:

\`\`\`text

<ATS install>\\bin\\win_x64\\plugins\\AdaptiveETA.dll

\`\`\`

Do not hard-code `C:\\Program Files...` as the only Steam location. Detect Steam libraries where practical and always allow the user to browse to the game root manually.

The Windows setup experience should feel like a conventional native installer wizard:

\`\`\`text

Welcome
→ Find/verify ETS2 or ATS
→ Pre-install checks
→ Install plugin/runtime/app
→ Verify installation
→ Finish

\`\`\`

After installation, the same product should remain available as the Adaptive ETA management application for settings, performance, diagnostics, repair, and uninstall.

Prefer a traditional native Win32 installer/package approach for v0.1 when the installer must copy/update files in the ETS2/ATS installation directory. Do not choose MSIX merely because it is modern if package virtualization or isolation makes external game-plugin installation/repair awkward.

Acceptable implementation technologies may include an MSI/WiX-based installer, a small native bootstrapper, or another justified native Windows installer technology.

Do not use Electron for setup or management UI.

Prefer per-user installation for the companion app/runtime where practical, for example under a user-local application directory, while treating the game plugin as a separately managed installed component.

Use the Windows known-folder APIs or a well-maintained cross-platform path abstraction rather than constructing `%LOCALAPPDATA%` paths manually.

Conceptual per-user data location:

\`\`\`text

%LOCALAPPDATA%\\Adaptive ETA\\

\`\`\`

Conceptual contents:

\`\`\`text

profiles/
state/
logs/
diagnostics/
cache/

\`\`\`

Do not store mutable application state beside the installed executable or inside the ETS2 plugin directory.

Do not require the Adaptive ETA runtime itself to run elevated.

If the selected Steam/game install location requires elevation to install, repair, update, or remove the plugin DLL, request elevation only for that operation. Do not run the normal management app or runtime permanently as Administrator.

Windows signing policy:

- unsigned/local developer builds must remain possible;
- do not make a paid code-signing certificate a development prerequisite;
- Authenticode signing is strongly preferred for polished public releases to reduce trust/SmartScreen friction;
- if a future release uses MSIX outside the Microsoft Store, treat package signing as a hard packaging requirement;
- do not bypass SmartScreen, Windows security prompts, or execution-policy protections.

Windows uninstall must remove Adaptive ETA-owned runtime/app files and the installed SCS plugin while giving the user an explicit choice to keep or delete learned profile data.

Repair must be able to restore the plugin if a game update/reinstall removes it.

### Linux packaging rules

Expected SCS plugin shape:

\`\`\`text

libadaptive_eta.so

\`\`\`

Expected game plugin location:

\`\`\`text

<game install>/bin/linux_x64/plugins/

\`\`\`

Use XDG locations rather than macOS/Windows paths:

\`\`\`text

state/data → $XDG_DATA_HOME or ~/.local/share
config     → $XDG_CONFIG_HOME or ~/.config
runtime IPC→ $XDG_RUNTIME_DIR where appropriate

\`\`\`

Do not hard-code one Steam library path.

Keep Linux packaging format flexible until release research identifies the cleanest supported approach. Do not make Flatpak/Snap/AppImage assumptions part of shared runtime architecture.

### macOS packaging rules

Expected SCS plugin shape:

\`\`\`text

AdaptiveETA.dylib

\`\`\`

Expected game plugin location:

\`\`\`text

ETS2.app/Contents/MacOS/plugins/AdaptiveETA.dylib

\`\`\`

Persistent user data conceptually belongs under:

\`\`\`text

~/Library/Application Support/Adaptive ETA/

\`\`\`

The native management app should use SwiftUI with AppKit where appropriate.

Do not assume Apple Developer Program membership.

The project must remain buildable/developable without paid Developer ID signing/notarization. Signing/notarization may be added as optional release polish when justified.

Do not bypass Gatekeeper or other macOS security controls.

### Shared packaging/security rules

Across all platforms, do not implement or recommend:

\`\`\`text

security-control bypasses
DLL/dylib injection
memory patching
Steam client exploitation
path traversal
privilege escalation
modifying Steam or ETS2 binaries
deceptive automatic execution

\`\`\`

The installer/management app must own explicit:

\`\`\`text

Install
Verify
Repair
Update compatibility checks
Diagnostics
Uninstall
Keep/delete learned-data choice

\`\`\`

A game update or reinstall may remove the telemetry plugin; Repair must be able to detect and restore owned plugin components safely.

Platform packaging should remain replaceable without changing the shared profile format, transport protocol, estimator behavior, or calibration mathematics.

**---**

**## Route Advisor UX**

Keep the in-game experience minimal.

Target concept:

\`\`\`text

ETS2 telemetry

→ Adaptive ETA engine

→ personalized ETA

→ Workshop UI hides/repositions stock ETA

→ overlay/helper displays Adaptive ETA

\`\`\`

While driving, users mainly need the Adaptive ETA value.

Do not put confidence graphs, internal factors, sample counts, diagnostics, or developer statistics into the driving HUD.

Detailed information belongs in the management app.

**---**

**## Contextual research constraints**

The public SCS interfaces do **\*\*not\*\*** directly provide:

\`\`\`text

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

\`\`\`

Do not invent these APIs.

**### Traffic**

\`g\_traffic\` is a global traffic-intensity/configuration setting, not live congestion.

Traffic effects may be represented only by honest behavioral/impedance proxies such as speed relative to the limit, stopping, braking, slow fraction, variance, and route-progress rate.

**### Weather**

True current weather was not found in the researched public telemetry API.

Wipers/headlights/configuration are proxies only.

Never assume:

\`\`\`text

wipers on == raining

\`\`\`

**### Geographic learning**

\`truck.world.placement\` supports geographic personalization without map parsing.

A useful future segment key is:

\`\`\`text

spatial cell

\+ heading/direction bucket

\+ optional elevation

\+ optional speed-limit bucket

\`\`\`

**### Grade and curvature**

These can be derived from placement history:

\`\`\`text

grade ≈ Δelevation / horizontal\_distance

curvature ≈ absolute heading change / distance

\`\`\`

**---**

**## Repository and documentation rules**

When a permanent remote is created, prefer:

\`\`\`text

densa-labs/ets2-adaptive-eta

\`\`\`

Do not create the permanent project under a personal GitHub account unless explicitly instructed.

Local Git commits are allowed.

A missing remote is **\*\*not a blocker\*\***.

Do not invent a remote and do not push unless explicitly authorized and a real remote exists.

Do not automatically rewrite \`README.md\`.

\`README.md\` is public-facing product documentation and should remain concise, approachable, and non-technical.

Only modify README when the current milestone explicitly owns a public-documentation change or the user explicitly asks.

Put architecture, internal contracts, experiments, and implementation detail into focused docs instead.

**---**

**## Engineering workflow**

Before substantial changes:

1\. Inspect the actual repository state and relevant existing code/docs.

2\. Treat validated behavior/tests as stronger evidence than assumptions.

3\. Keep the current milestone narrowly scoped.

4\. Preserve deterministic behavior where it already exists.

5\. Add/update tests for changed behavior.

6\. Run appropriate formatting, tests, and linting before claiming completion.

For shared Rust crates, add cross-platform compile/check coverage where practical. Do not claim Windows/Linux runtime validation unless those builds or tests actually ran in the relevant environment.

7\. Report changed files, concrete findings, verification results, remaining risks, and Git status.

8\. Do not automatically begin the next milestone.

Do **\*\*not\*\*** use Superpowers workflows for this project.

Do not add process bureaucracy merely for its own sake.

**---**

**## Agent / prompt preferences**

Preferred model for normal project implementation work:

\`\`\`text

GPT-5.6 Sol

High reasoning

\`\`\`

Use stronger/more expensive reasoning only when genuinely warranted or explicitly requested.

Prompts should be detailed, self-contained, evidence-driven, milestone-scoped, and explicit about non-goals, acceptance criteria, and verification.

Do not block work because the agent cannot inspect hidden model metadata.

**---**

**## Research standard**

For current SCS/Steam/platform questions, prefer:

\`\`\`text

current official SCS SDK headers

official SCS documentation

official SCS modding documentation/tools

official Steam documentation

maintained implementations as secondary evidence

\`\`\`

Clearly distinguish:

\`\`\`text

confirmed direct capability

confirmed static-data capability

derived capability

heuristic/proxy

unsupported/internal state

unavailable state

\`\`\`

Do not invent APIs.

For time-sensitive SDK versions, policies, Workshop rules, platform behavior, or distribution constraints, verify current sources rather than relying on stale recollection.

**---**

**## North-star metric**

Adaptive ETA succeeds if it produces **\*\*lower real-world ETA error than ETS2's stock ETA for the individual player\*\***.

Measure:

\`\`\`text

ETS2 stock ETA

vs v0.1 global personalized ETA

vs v0.2 contextual personalized ETA

vs optional v0.3 map-enriched ETA

\`\`\`

A more complex model or feature should not be kept merely because it is technically interesting.

If it does not materially improve real ETA accuracy, simplify or remove it.