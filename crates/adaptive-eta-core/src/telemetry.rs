use crate::lifecycle::LifecycleEvent;

/// One adapter-normalized observation. Both clocks are monotonic within a
/// source epoch and exclude paused time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TelemetryFrame {
    pub sequence: u64,
    pub source_epoch: u64,
    /// Integrated scaled game seconds; used only as the calibration numerator.
    pub active_game_time_sec: f64,
    /// Unscaled active simulation seconds; used for gaps, stabilization, and AFK.
    pub active_physical_time_sec: f64,
    pub navigation_distance_m: f64,
    pub navigation_time_sec: f64,
    pub odometer_km: f64,
    pub speed_mps: f64,
    pub navigation_valid: bool,
}

impl TelemetryFrame {
    #[must_use]
    pub fn values_are_finite(self) -> bool {
        self.active_game_time_sec.is_finite()
            && self.active_physical_time_sec.is_finite()
            && self.navigation_distance_m.is_finite()
            && self.navigation_time_sec.is_finite()
            && self.odometer_km.is_finite()
            && self.speed_mps.is_finite()
    }

    #[must_use]
    pub fn values_are_nonnegative(self) -> bool {
        self.active_game_time_sec >= 0.0
            && self.active_physical_time_sec >= 0.0
            && self.navigation_distance_m >= 0.0
            && self.navigation_time_sec >= 0.0
            && self.odometer_km >= 0.0
    }

    #[must_use]
    pub fn is_usable(self) -> bool {
        self.navigation_valid && self.values_are_finite() && self.values_are_nonnegative()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EngineInput {
    Frame(TelemetryFrame),
    Event(LifecycleEvent),
}
