pub const DISTANCE_CONFIDENCE_KM: f64 = 200.0;
pub const SAMPLE_CONFIDENCE_COUNT: f64 = 20.0;

#[must_use]
pub fn confidence(observed_distance_km: f64, sample_count: u64) -> f64 {
    if !observed_distance_km.is_finite() || observed_distance_km < 0.0 {
        return 0.0;
    }

    let distance = observed_distance_km / (observed_distance_km + DISTANCE_CONFIDENCE_KM);
    let count_value = f64::from(u32::try_from(sample_count).unwrap_or(u32::MAX));
    let count = count_value / (count_value + SAMPLE_CONFIDENCE_COUNT);
    distance.min(count).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::confidence;

    #[test]
    fn confidence_is_zero_without_evidence() {
        assert!(confidence(0.0, 0).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_distance_cannot_escape_confidence_bounds() {
        assert!(confidence(f64::NAN, u64::MAX).abs() < f64::EPSILON);
        assert!(confidence(-1.0, u64::MAX).abs() < f64::EPSILON);
        assert!((0.0..=1.0).contains(&confidence(f64::MAX, u64::MAX)));
    }
}
