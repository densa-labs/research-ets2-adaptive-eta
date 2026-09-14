use crate::confidence::confidence;
use crate::diagnostics::UpdateDiagnostic;
use serde::{Deserialize, Serialize};

pub const MAX_SAMPLE_WEIGHT_KM: f64 = 12.5;
pub const EWMA_DISTANCE_SCALE_KM: f64 = 250.0;
pub const MAX_ALPHA: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstimatorView {
    pub learned_factor: f64,
    pub confidence: f64,
    pub display_factor: f64,
    pub valid_sample_count: u64,
    pub valid_observed_distance_km: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EstimatorState {
    log_factor: f64,
    valid_sample_count: u64,
    valid_observed_distance_km: f64,
    rejected_invalid: u64,
    rejected_discontinuity: u64,
    rejected_outlier: u64,
    bounded_sample_count: u64,
}

impl Default for EstimatorState {
    fn default() -> Self {
        Self {
            log_factor: 0.0,
            valid_sample_count: 0,
            valid_observed_distance_km: 0.0,
            rejected_invalid: 0,
            rejected_discontinuity: 0,
            rejected_outlier: 0,
            bounded_sample_count: 0,
        }
    }
}

impl EstimatorState {
    #[must_use]
    pub fn view(&self) -> EstimatorView {
        let learned_factor = self.log_factor.exp();
        let confidence = confidence(self.valid_observed_distance_km, self.valid_sample_count);
        let display_factor = 1.0 + confidence * (learned_factor - 1.0);
        EstimatorView {
            learned_factor,
            confidence,
            display_factor,
            valid_sample_count: self.valid_sample_count,
            valid_observed_distance_km: self.valid_observed_distance_km,
        }
    }

    #[must_use]
    pub fn adaptive_eta_sec(&self, game_eta_sec: f64) -> Option<f64> {
        if !game_eta_sec.is_finite() || game_eta_sec < 0.0 {
            return None;
        }
        let result = game_eta_sec * self.view().display_factor;
        result.is_finite().then_some(result)
    }

    pub(crate) fn accept(
        &mut self,
        ratio: f64,
        distance_driven_km: f64,
        bounded: bool,
    ) -> UpdateDiagnostic {
        let weight_km = distance_driven_km.min(MAX_SAMPLE_WEIGHT_KM);
        let alpha = MAX_ALPHA.min(1.0 - (-weight_km / EWMA_DISTANCE_SCALE_KM).exp());
        let old_factor = self.log_factor.exp();
        self.log_factor += alpha * (ratio.ln() - self.log_factor);
        self.valid_sample_count = self.valid_sample_count.saturating_add(1);
        self.valid_observed_distance_km += distance_driven_km;
        if bounded {
            self.bounded_sample_count = self.bounded_sample_count.saturating_add(1);
        }
        UpdateDiagnostic {
            applied_ratio: ratio,
            weight_km,
            alpha,
            old_factor,
            new_factor: self.log_factor.exp(),
        }
    }

    pub(crate) fn reject_invalid(&mut self) {
        self.rejected_invalid = self.rejected_invalid.saturating_add(1);
    }

    pub(crate) fn reject_outlier(&mut self) {
        self.rejected_outlier = self.rejected_outlier.saturating_add(1);
    }

    pub(crate) fn reject_discontinuity(&mut self) {
        self.rejected_discontinuity = self.rejected_discontinuity.saturating_add(1);
    }

    #[must_use]
    pub const fn rejected_invalid(&self) -> u64 {
        self.rejected_invalid
    }

    #[must_use]
    pub const fn rejected_discontinuity(&self) -> u64 {
        self.rejected_discontinuity
    }

    #[must_use]
    pub const fn rejected_outlier(&self) -> u64 {
        self.rejected_outlier
    }

    #[must_use]
    pub const fn bounded_sample_count(&self) -> u64 {
        self.bounded_sample_count
    }
}

#[cfg(test)]
mod tests {
    use super::EstimatorState;

    #[test]
    fn zero_confidence_preserves_game_eta_exactly() {
        let estimator = EstimatorState::default();
        assert_eq!(estimator.adaptive_eta_sec(12_345.0), Some(12_345.0));
    }

    #[test]
    fn invalid_eta_is_not_transformed() {
        let estimator = EstimatorState::default();
        assert_eq!(estimator.adaptive_eta_sec(f64::NAN), None);
        assert_eq!(estimator.adaptive_eta_sec(f64::INFINITY), None);
        assert_eq!(estimator.adaptive_eta_sec(-1.0), None);
    }
}
