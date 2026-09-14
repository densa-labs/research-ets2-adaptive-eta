use crate::lifecycle::BoundaryReason;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleMetrics {
    pub start_navigation_distance_m: f64,
    pub end_navigation_distance_m: f64,
    pub start_navigation_time_sec: f64,
    pub end_navigation_time_sec: f64,
    pub start_odometer_km: f64,
    pub end_odometer_km: f64,
    pub start_active_game_time_sec: f64,
    pub end_active_game_time_sec: f64,
    pub distance_driven_km: f64,
    pub route_progress_km: f64,
    pub predicted_consumed_sec: f64,
    pub actual_consumed_sec: f64,
    pub raw_ratio: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectionReason {
    InvalidNumber,
    NonPositiveDistance,
    NonPositiveRouteProgress,
    InsufficientPredictedProgress,
    NonPositiveActualTime,
    DistanceProgressMismatch,
    Outlier,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SampleOutcome {
    AcceptedUnchanged { ratio: f64 },
    AcceptedBounded { raw_ratio: f64, bounded_ratio: f64 },
    Rejected { reason: RejectionReason },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateDiagnostic {
    pub applied_ratio: f64,
    pub weight_km: f64,
    pub alpha: f64,
    pub old_factor: f64,
    pub new_factor: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SampleDecision {
    pub metrics: SampleMetrics,
    pub outcome: SampleOutcome,
    pub update: Option<UpdateDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoundaryDiagnostic {
    pub reason: BoundaryReason,
    pub discarded_open_window: bool,
    /// Progress accumulated through the last trusted frame before the boundary.
    pub discarded_metrics: Option<SampleMetrics>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EngineOutput {
    Boundary(BoundaryDiagnostic),
    AnchorEstablished { sequence: u64 },
    Sample(SampleDecision),
}
