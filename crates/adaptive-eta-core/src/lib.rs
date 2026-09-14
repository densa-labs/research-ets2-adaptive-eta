//! Deterministic, platform-neutral calibration engine for Adaptive ETA.
//!
//! A sample compares scaled active game time consumed with the decrease in
//! ETS2's remaining navigation time over one uninterrupted telemetry epoch.
//! The core never reads clocks or files: normalized frames and lifecycle
//! boundaries are its only inputs.
//!
//! Accepted ratios update a distance-aware EWMA in log space. The learned
//! factor records the evidence directly; the displayed factor blends it toward
//! 1.0 while confidence is low. Boundaries discard open evidence and require a
//! fresh stabilization period, favoring false rejection over contaminated
//! calibration.

pub mod confidence;
pub mod diagnostics;
pub mod estimator;
pub mod lifecycle;
pub mod sampler;
pub mod telemetry;

pub use diagnostics::{
    BoundaryDiagnostic, EngineOutput, RejectionReason, SampleDecision, SampleMetrics,
    SampleOutcome, UpdateDiagnostic,
};
pub use estimator::{
    CALIBRATION_MODEL_VERSION, CalibrationSnapshot, EstimatorState, EstimatorView,
    SnapshotValidationError,
};
pub use lifecycle::{BoundaryReason, LifecycleEvent};
pub use sampler::{CalibrationEngine, EngineConfig};
pub use telemetry::{EngineInput, TelemetryFrame};
