//! Percentile summaries for repeated latency measurements.

/// One bounded latency summary over repeated runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatencySummary {
    /// Smallest observed offset in milliseconds.
    pub min_ms: f64,
    /// Median observed offset in milliseconds.
    pub p50_ms: f64,
    /// 95th-percentile observed offset in milliseconds.
    pub p95_ms: f64,
    /// 99th-percentile observed offset in milliseconds.
    pub p99_ms: f64,
    /// Largest observed offset in milliseconds.
    pub max_ms: f64,
}

/// Returns the linear-interpolated `percentile` of a pre-sorted slice.
///
/// `percentile` must be in `0.0..=100.0`. A single value is returned for
/// single-element inputs, matching the numpy-default interpolation convention.
///
/// # Panics
///
/// Panics when `sorted` is empty or `percentile` is outside `0.0..=100.0`.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    assert!(!sorted.is_empty(), "percentile requires at least one value");
    assert!(
        (0.0..=100.0).contains(&percentile),
        "percentile must be in 0.0..=100.0"
    );
    if sorted.len() == 1 {
        return sorted[0];
    }
    let position = percentile / 100.0 * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let weight = position - lower as f64;
    sorted[lower] * (1.0 - weight) + sorted[upper] * weight
}

/// Summarizes unsorted per-run offsets in milliseconds.
///
/// Returns `None` when no offsets were provided or when any offset is not a
/// finite number, so a failed run cannot silently produce a NaN quantile.
#[must_use]
pub fn summarize_ms(offsets_ms: &[f64]) -> Option<LatencySummary> {
    if offsets_ms.is_empty() || offsets_ms.iter().any(|offset| !offset.is_finite()) {
        return None;
    }
    let mut sorted = offsets_ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(LatencySummary {
        min_ms: sorted[0],
        p50_ms: percentile(&sorted, 50.0),
        p95_ms: percentile(&sorted, 95.0),
        p99_ms: percentile(&sorted, 99.0),
        max_ms: sorted[sorted.len() - 1],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_value_is_returned_for_any_percentile() {
        assert!((percentile(&[3.0], 50.0) - 3.0).abs() < 1e-12);
        assert!((percentile(&[3.0], 99.0) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn median_of_even_count_is_interpolated() {
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0], 50.0) - 2.5).abs() < 1e-12);
    }

    #[test]
    fn extremes_are_returned_for_zero_and_full_percentiles() {
        let values = [2.0, 5.0, 9.0];
        assert!((percentile(&values, 0.0) - 2.0).abs() < 1e-12);
        assert!((percentile(&values, 100.0) - 9.0).abs() < 1e-12);
    }

    #[test]
    fn summary_reports_bounds_and_quantiles() {
        let summary = summarize_ms(&[4.0, 1.0, 3.0, 2.0]).expect("values present");
        assert!((summary.min_ms - 1.0).abs() < 1e-9);
        assert!((summary.max_ms - 4.0).abs() < 1e-9);
        assert!((summary.p50_ms - 2.5).abs() < 1e-9);
        assert!((summary.p95_ms - 3.85).abs() < 1e-9);
        assert!((summary.p99_ms - 3.97).abs() < 1e-9);
    }

    #[test]
    fn empty_input_has_no_summary() {
        assert!(summarize_ms(&[]).is_none());
    }

    #[test]
    fn non_finite_inputs_have_no_summary() {
        assert!(summarize_ms(&[f64::NAN, 1.0]).is_none());
        assert!(summarize_ms(&[f64::INFINITY]).is_none());
        assert!(summarize_ms(&[f64::NEG_INFINITY, -1.0]).is_none());
        assert!(summarize_ms(&[2.0, f64::NAN, 1.0]).is_none());
    }
}
