use crate::diagnostics::{
    BoundaryDiagnostic, EngineOutput, RejectionReason, SampleDecision, SampleMetrics, SampleOutcome,
};
use crate::estimator::{CalibrationSnapshot, EstimatorState, SnapshotValidationError};
use crate::lifecycle::{BoundaryReason, LifecycleEvent};
use crate::telemetry::{EngineInput, TelemetryFrame};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineConfig {
    pub target_distance_km: f64,
    pub minimum_active_game_time_sec: f64,
    pub minimum_predicted_consumed_sec: f64,
    pub stabilization_physical_time_sec: f64,
    pub stationary_speed_mps: f64,
    pub maximum_stationary_physical_time_sec: f64,
    pub maximum_frame_physical_gap_sec: f64,
    pub maximum_sequence_gap: u64,
    pub navigation_distance_increase_tolerance_m: f64,
    pub navigation_time_increase_tolerance_sec: f64,
    pub odometer_regression_tolerance_km: f64,
    pub progress_base_tolerance_km: f64,
    pub progress_relative_tolerance: f64,
    pub hard_ratio_min: f64,
    pub hard_ratio_max: f64,
    pub bounded_ratio_min: f64,
    pub bounded_ratio_max: f64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            target_distance_km: 8.0,
            minimum_active_game_time_sec: 4.0 * 60.0,
            minimum_predicted_consumed_sec: 3.0 * 60.0,
            stabilization_physical_time_sec: 5.0,
            stationary_speed_mps: 0.5,
            maximum_stationary_physical_time_sec: 120.0,
            maximum_frame_physical_gap_sec: 5.0,
            maximum_sequence_gap: 120,
            navigation_distance_increase_tolerance_m: 100.0,
            navigation_time_increase_tolerance_sec: 30.0,
            odometer_regression_tolerance_km: 0.01,
            progress_base_tolerance_km: 0.75,
            progress_relative_tolerance: 0.25,
            hard_ratio_min: 0.50,
            hard_ratio_max: 1.75,
            bounded_ratio_min: 0.67,
            bounded_ratio_max: 1.50,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CalibrationEngine {
    config: EngineConfig,
    estimator: EstimatorState,
    anchor: Option<TelemetryFrame>,
    stabilization_start: Option<TelemetryFrame>,
    last_frame: Option<TelemetryFrame>,
    stationary_since_physical_sec: Option<f64>,
    source_epoch: Option<u64>,
    source_disconnected: bool,
    awaiting_movement_after_afk: bool,
}

impl Default for CalibrationEngine {
    fn default() -> Self {
        Self::new(EngineConfig::default())
    }
}

impl CalibrationEngine {
    #[must_use]
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            estimator: EstimatorState::default(),
            anchor: None,
            stabilization_start: None,
            last_frame: None,
            stationary_since_physical_sec: None,
            source_epoch: None,
            source_disconnected: false,
            awaiting_movement_after_afk: false,
        }
    }

    /// Creates an engine with durable estimator evidence and clean transient
    /// sampling/lifecycle state.
    ///
    /// # Errors
    ///
    /// Returns an error if the durable snapshot is invalid or unsupported.
    pub fn from_snapshot(
        config: EngineConfig,
        snapshot: CalibrationSnapshot,
    ) -> Result<Self, SnapshotValidationError> {
        let mut engine = Self::new(config);
        engine.estimator = EstimatorState::from_snapshot(snapshot)?;
        Ok(engine)
    }

    #[must_use]
    pub const fn config(&self) -> &EngineConfig {
        &self.config
    }

    #[must_use]
    pub const fn estimator(&self) -> &EstimatorState {
        &self.estimator
    }

    pub fn process(&mut self, input: EngineInput) -> Option<EngineOutput> {
        match input {
            EngineInput::Event(event) => Some(self.process_event(event)),
            EngineInput::Frame(frame) => self.process_frame(frame),
        }
    }

    fn process_event(&mut self, event: LifecycleEvent) -> EngineOutput {
        match event {
            LifecycleEvent::SourceConnected { source_epoch } => {
                self.source_epoch = Some(source_epoch);
                self.source_disconnected = false;
            }
            LifecycleEvent::SourceDisconnected => self.source_disconnected = true,
            _ => {}
        }
        self.apply_boundary(event.boundary_reason())
    }

    fn process_frame(&mut self, frame: TelemetryFrame) -> Option<EngineOutput> {
        if self.source_disconnected {
            return None;
        }
        if !frame.navigation_valid {
            return Some(self.boundary_with_frame(BoundaryReason::NavigationInvalid, None));
        }
        if !frame.values_are_finite() || !frame.values_are_nonnegative() {
            return Some(self.boundary_with_frame(BoundaryReason::InvalidTelemetry, None));
        }

        if self
            .source_epoch
            .is_some_and(|epoch| epoch != frame.source_epoch)
        {
            self.source_epoch = Some(frame.source_epoch);
            return Some(self.boundary_with_frame(BoundaryReason::SourceEpochChanged, Some(frame)));
        }
        self.source_epoch.get_or_insert(frame.source_epoch);

        if let Some(previous) = self.last_frame
            && let Some(reason) = self.frame_discontinuity(previous, frame)
        {
            return Some(self.boundary_with_frame(reason, Some(frame)));
        }

        if self.awaiting_movement_after_afk {
            self.last_frame = Some(frame);
            if frame.speed_mps.abs() < self.config.stationary_speed_mps {
                return None;
            }
            self.awaiting_movement_after_afk = false;
            self.stabilization_start = Some(frame);
            self.stationary_since_physical_sec = None;
            return None;
        }

        if frame.speed_mps.abs() < self.config.stationary_speed_mps {
            let since = self
                .stationary_since_physical_sec
                .get_or_insert(frame.active_physical_time_sec);
            if frame.active_physical_time_sec - *since
                >= self.config.maximum_stationary_physical_time_sec
            {
                return Some(self.boundary_with_frame(BoundaryReason::LongStationary, Some(frame)));
            }
        } else {
            self.stationary_since_physical_sec = None;
        }

        self.last_frame = Some(frame);

        let Some(anchor) = self.anchor else {
            let start = *self.stabilization_start.get_or_insert(frame);
            if frame.active_physical_time_sec - start.active_physical_time_sec
                >= self.config.stabilization_physical_time_sec
            {
                self.anchor = Some(frame);
                self.stabilization_start = None;
                return Some(EngineOutput::AnchorEstablished {
                    sequence: frame.sequence,
                });
            }
            return None;
        };

        let metrics = sample_metrics(anchor, frame);
        if metrics.distance_driven_km < self.config.target_distance_km
            || metrics.actual_consumed_sec < self.config.minimum_active_game_time_sec
        {
            return None;
        }

        let decision = self.decide_sample(metrics);
        self.anchor = Some(frame);
        Some(EngineOutput::Sample(decision))
    }

    fn frame_discontinuity(
        &self,
        previous: TelemetryFrame,
        current: TelemetryFrame,
    ) -> Option<BoundaryReason> {
        if current.sequence <= previous.sequence {
            return Some(BoundaryReason::SequenceRegressed);
        }
        if current.active_game_time_sec < previous.active_game_time_sec
            || current.active_physical_time_sec < previous.active_physical_time_sec
        {
            return Some(BoundaryReason::TimestampRegressed);
        }
        if current.sequence - previous.sequence > self.config.maximum_sequence_gap
            || current.active_physical_time_sec - previous.active_physical_time_sec
                > self.config.maximum_frame_physical_gap_sec
        {
            return Some(BoundaryReason::TelemetryGap);
        }

        let odometer_delta_km = current.odometer_km - previous.odometer_km;
        if odometer_delta_km < -self.config.odometer_regression_tolerance_km {
            return Some(BoundaryReason::OdometerRegressed);
        }
        if current.navigation_distance_m - previous.navigation_distance_m
            > self.config.navigation_distance_increase_tolerance_m
        {
            return Some(BoundaryReason::NavigationDistanceIncreased);
        }
        if current.navigation_time_sec - previous.navigation_time_sec
            > self.config.navigation_time_increase_tolerance_sec
        {
            return Some(BoundaryReason::NavigationTimeIncreased);
        }

        let route_progress_km =
            (previous.navigation_distance_m - current.navigation_distance_m) / 1_000.0;
        if route_progress_km > 0.0
            && !progress_is_consistent(route_progress_km, odometer_delta_km.max(0.0), &self.config)
        {
            return Some(BoundaryReason::ProgressMismatch);
        }
        None
    }

    fn decide_sample(&mut self, metrics: SampleMetrics) -> SampleDecision {
        let rejection = if !sample_values_are_finite(&metrics) {
            Some(RejectionReason::InvalidNumber)
        } else if metrics.distance_driven_km <= 0.0 {
            Some(RejectionReason::NonPositiveDistance)
        } else if metrics.route_progress_km <= 0.0 {
            Some(RejectionReason::NonPositiveRouteProgress)
        } else if metrics.predicted_consumed_sec < self.config.minimum_predicted_consumed_sec {
            Some(RejectionReason::InsufficientPredictedProgress)
        } else if metrics.actual_consumed_sec <= 0.0 {
            Some(RejectionReason::NonPositiveActualTime)
        } else if !progress_is_consistent(
            metrics.route_progress_km,
            metrics.distance_driven_km,
            &self.config,
        ) {
            Some(RejectionReason::DistanceProgressMismatch)
        } else {
            None
        };

        if let Some(reason) = rejection {
            self.estimator.reject_invalid();
            return SampleDecision {
                metrics,
                outcome: SampleOutcome::Rejected { reason },
                update: None,
            };
        }

        let raw_ratio = metrics.raw_ratio.expect("validated positive denominator");
        if !(self.config.hard_ratio_min..=self.config.hard_ratio_max).contains(&raw_ratio) {
            self.estimator.reject_outlier();
            return SampleDecision {
                metrics,
                outcome: SampleOutcome::Rejected {
                    reason: RejectionReason::Outlier,
                },
                update: None,
            };
        }

        let bounded_ratio =
            raw_ratio.clamp(self.config.bounded_ratio_min, self.config.bounded_ratio_max);
        let bounded =
            raw_ratio < self.config.bounded_ratio_min || raw_ratio > self.config.bounded_ratio_max;
        let update = self
            .estimator
            .accept(bounded_ratio, metrics.distance_driven_km, bounded);
        let outcome = if bounded {
            SampleOutcome::AcceptedBounded {
                raw_ratio,
                bounded_ratio,
            }
        } else {
            SampleOutcome::AcceptedUnchanged { ratio: raw_ratio }
        };
        SampleDecision {
            metrics,
            outcome,
            update: Some(update),
        }
    }

    fn apply_boundary(&mut self, reason: BoundaryReason) -> EngineOutput {
        let discarded_metrics = self
            .anchor
            .take()
            .zip(self.last_frame)
            .map(|(anchor, last)| sample_metrics(anchor, last));
        let discarded_open_window = discarded_metrics.is_some();
        if discarded_open_window {
            self.estimator.reject_discontinuity();
        }
        self.stabilization_start = None;
        self.last_frame = None;
        self.stationary_since_physical_sec = None;
        self.awaiting_movement_after_afk = false;
        EngineOutput::Boundary(BoundaryDiagnostic {
            reason,
            discarded_open_window,
            discarded_metrics,
        })
    }

    fn boundary_with_frame(
        &mut self,
        reason: BoundaryReason,
        stabilization_frame: Option<TelemetryFrame>,
    ) -> EngineOutput {
        let output = self.apply_boundary(reason);
        self.awaiting_movement_after_afk = reason == BoundaryReason::LongStationary;
        if let Some(frame) = stabilization_frame.filter(|frame| frame.is_usable()) {
            self.last_frame = Some(frame);
            self.stabilization_start = Some(frame);
            if frame.speed_mps.abs() < self.config.stationary_speed_mps {
                self.stationary_since_physical_sec = Some(frame.active_physical_time_sec);
            }
        }
        output
    }
}

fn sample_metrics(start: TelemetryFrame, end: TelemetryFrame) -> SampleMetrics {
    let distance_driven_km = end.odometer_km - start.odometer_km;
    let route_progress_km = (start.navigation_distance_m - end.navigation_distance_m) / 1_000.0;
    let predicted_consumed_sec = start.navigation_time_sec - end.navigation_time_sec;
    let actual_consumed_sec = end.active_game_time_sec - start.active_game_time_sec;
    let raw_ratio = (predicted_consumed_sec > 0.0 && actual_consumed_sec > 0.0)
        .then(|| actual_consumed_sec / predicted_consumed_sec)
        .filter(|ratio| ratio.is_finite());
    SampleMetrics {
        start_navigation_distance_m: start.navigation_distance_m,
        end_navigation_distance_m: end.navigation_distance_m,
        start_navigation_time_sec: start.navigation_time_sec,
        end_navigation_time_sec: end.navigation_time_sec,
        start_odometer_km: start.odometer_km,
        end_odometer_km: end.odometer_km,
        start_active_game_time_sec: start.active_game_time_sec,
        end_active_game_time_sec: end.active_game_time_sec,
        distance_driven_km,
        route_progress_km,
        predicted_consumed_sec,
        actual_consumed_sec,
        raw_ratio,
    }
}

fn sample_values_are_finite(metrics: &SampleMetrics) -> bool {
    metrics.start_navigation_distance_m.is_finite()
        && metrics.end_navigation_distance_m.is_finite()
        && metrics.start_navigation_time_sec.is_finite()
        && metrics.end_navigation_time_sec.is_finite()
        && metrics.start_odometer_km.is_finite()
        && metrics.end_odometer_km.is_finite()
        && metrics.start_active_game_time_sec.is_finite()
        && metrics.end_active_game_time_sec.is_finite()
        && metrics.distance_driven_km.is_finite()
        && metrics.route_progress_km.is_finite()
        && metrics.predicted_consumed_sec.is_finite()
        && metrics.actual_consumed_sec.is_finite()
        && metrics.raw_ratio.is_none_or(f64::is_finite)
}

fn progress_is_consistent(
    route_progress_km: f64,
    distance_driven_km: f64,
    config: &EngineConfig,
) -> bool {
    (route_progress_km - distance_driven_km).abs()
        <= config.progress_base_tolerance_km
            + config.progress_relative_tolerance * distance_driven_km
}
