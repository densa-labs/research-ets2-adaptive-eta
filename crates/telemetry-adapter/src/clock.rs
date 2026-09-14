use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClockIssue {
    TimerRestart,
    TimestampRegression { previous_us: u64, current_us: u64 },
    MissingIntervalScale,
    InvalidScale { value: f64 },
    PausedTimestampAdvanced { delta_us: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClockObservation {
    pub active_game_time_sec: f64,
    pub active_physical_time_sec: f64,
    pub delta_physical_time_sec: f64,
    pub interval_scale: Option<f64>,
    pub issue: Option<ClockIssue>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IntervalClock {
    previous_timestamp_us: Option<u64>,
    previous_scale: Option<f64>,
    active_game_time_sec: f64,
    active_physical_time_sec: f64,
}

impl IntervalClock {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn observe(
        &mut self,
        timestamp_us: u64,
        current_scale: Option<f64>,
        paused: bool,
        timer_restart: bool,
    ) -> ClockObservation {
        if timer_restart {
            self.reset_to_baseline(timestamp_us, valid_scale(current_scale));
            return self.observation(0.0, None, Some(ClockIssue::TimerRestart));
        }

        let invalid_scale = current_scale.filter(|scale| !is_valid_scale(*scale));
        if let Some(value) = invalid_scale {
            self.reset_to_baseline(timestamp_us, None);
            return self.observation(0.0, None, Some(ClockIssue::InvalidScale { value }));
        }
        let current_scale = valid_scale(current_scale);

        let Some(previous_timestamp_us) = self.previous_timestamp_us else {
            self.reset_to_baseline(timestamp_us, current_scale);
            return self.observation(0.0, None, None);
        };

        let Some(delta_us) = timestamp_us.checked_sub(previous_timestamp_us) else {
            self.reset_to_baseline(timestamp_us, current_scale);
            return self.observation(
                0.0,
                None,
                Some(ClockIssue::TimestampRegression {
                    previous_us: previous_timestamp_us,
                    current_us: timestamp_us,
                }),
            );
        };

        if paused {
            self.previous_timestamp_us = Some(timestamp_us);
            self.previous_scale = current_scale;
            if delta_us > 0 {
                return self.observation(
                    0.0,
                    None,
                    Some(ClockIssue::PausedTimestampAdvanced { delta_us }),
                );
            }
            return self.observation(0.0, self.previous_scale, None);
        }

        if delta_us > 0 && self.previous_scale.is_none() {
            self.reset_to_baseline(timestamp_us, current_scale);
            return self.observation(0.0, None, Some(ClockIssue::MissingIntervalScale));
        }

        let delta_sec = std::time::Duration::from_micros(delta_us).as_secs_f64();
        let interval_scale = self.previous_scale;
        self.active_physical_time_sec += delta_sec;
        if let Some(scale) = interval_scale {
            self.active_game_time_sec += delta_sec * scale;
        }
        self.previous_timestamp_us = Some(timestamp_us);
        self.previous_scale = current_scale;
        self.observation(delta_sec, interval_scale, None)
    }

    fn reset_to_baseline(&mut self, timestamp_us: u64, scale: Option<f64>) {
        self.previous_timestamp_us = Some(timestamp_us);
        self.previous_scale = scale;
        self.active_game_time_sec = 0.0;
        self.active_physical_time_sec = 0.0;
    }

    fn observation(
        self,
        delta_physical_time_sec: f64,
        interval_scale: Option<f64>,
        issue: Option<ClockIssue>,
    ) -> ClockObservation {
        ClockObservation {
            active_game_time_sec: self.active_game_time_sec,
            active_physical_time_sec: self.active_physical_time_sec,
            delta_physical_time_sec,
            interval_scale,
            issue,
        }
    }
}

fn valid_scale(scale: Option<f64>) -> Option<f64> {
    scale.filter(|value| is_valid_scale(*value))
}

fn is_valid_scale(scale: f64) -> bool {
    scale.is_finite() && scale > 0.0
}

#[cfg(test)]
mod tests {
    use super::{ClockIssue, IntervalClock};

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1.0e-12);
    }

    #[test]
    fn first_frame_only_establishes_a_baseline() {
        let observation = IntervalClock::default().observe(5_000_000, Some(19.0), false, false);
        assert_close(observation.active_game_time_sec, 0.0);
        assert_close(observation.active_physical_time_sec, 0.0);
        assert_eq!(observation.issue, None);
    }

    #[test]
    fn constant_scale_integrates_both_clocks() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(0, Some(3.0), false, false);
        let observation = clock.observe(2_000_000, Some(3.0), false, false);
        assert_close(observation.active_physical_time_sec, 2.0);
        assert_close(observation.active_game_time_sec, 6.0);
    }

    #[test]
    fn scale_changes_use_the_previous_interval_scale() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(0, Some(3.0), false, false);
        let first = clock.observe(1_000_000, Some(19.0), false, false);
        let second = clock.observe(2_000_000, Some(3.0), false, false);
        assert_close(first.active_game_time_sec, 3.0);
        assert_close(second.active_game_time_sec, 22.0);
        assert_eq!(first.interval_scale, Some(3.0));
        assert_eq!(second.interval_scale, Some(19.0));
    }

    #[test]
    fn paused_observations_add_no_time() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(1_000_000, Some(3.0), false, false);
        let paused = clock.observe(1_000_000, Some(3.0), true, false);
        let resumed = clock.observe(2_000_000, Some(3.0), false, false);
        assert_close(paused.active_game_time_sec, 0.0);
        assert_close(paused.active_physical_time_sec, 0.0);
        assert_close(resumed.active_game_time_sec, 3.0);
        assert_close(resumed.active_physical_time_sec, 1.0);
    }

    #[test]
    fn timer_restart_resets_without_integrating_across_it() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(10_000_000, Some(19.0), false, false);
        let _ = clock.observe(11_000_000, Some(19.0), false, false);
        let restart = clock.observe(20_000, Some(3.0), false, true);
        assert_eq!(restart.issue, Some(ClockIssue::TimerRestart));
        assert_close(restart.active_game_time_sec, 0.0);
        assert_close(restart.active_physical_time_sec, 0.0);
    }

    #[test]
    fn timestamp_regression_resets_without_guessing() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(10_000_000, Some(3.0), false, false);
        let result = clock.observe(5_000_000, Some(3.0), false, false);
        assert!(matches!(
            result.issue,
            Some(ClockIssue::TimestampRegression { .. })
        ));
        assert_close(result.active_game_time_sec, 0.0);
    }

    #[test]
    fn missing_interval_scale_resets_without_guessing() {
        let mut clock = IntervalClock::default();
        let _ = clock.observe(0, None, false, false);
        let result = clock.observe(1_000_000, Some(3.0), false, false);
        assert_eq!(result.issue, Some(ClockIssue::MissingIntervalScale));
        assert_close(result.active_game_time_sec, 0.0);
    }

    #[test]
    fn invalid_scale_is_rejected() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let result = IntervalClock::default().observe(0, Some(scale), false, false);
            assert!(matches!(
                result.issue,
                Some(ClockIssue::InvalidScale { .. })
            ));
        }
    }
}
