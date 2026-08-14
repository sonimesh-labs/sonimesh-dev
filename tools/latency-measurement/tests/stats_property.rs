//! Deterministic property coverage for percentile summaries.
//!
//! `percentile` and `summarize_ms` must be a faithful quantile description of
//! the observed offsets: every quantile stays within the observed range and
//! hits the exact extrema at 0/100, quantiles move monotonically with the
//! requested percentile, the median matches the linear-interpolation
//! convention, and `summarize_ms` reports the true bounds with ordered
//! quantiles. A small seeded xorshift generator keeps the sweep deterministic
//! without adding a dependency.

use sonimesh_latency_measurement::{percentile, summarize_ms};

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    /// Uniform pseudo-random offset in `[-250.0, 250.0)`. A quarter of the
    /// draws use coarse half-millisecond steps so ties and duplicate values
    /// exercise the interpolation and `total_cmp` sort paths.
    #[allow(clippy::cast_precision_loss)]
    fn next_offset(&mut self) -> f64 {
        if self.next_u32() % 4 == 0 {
            f64::from(self.next_u32() % 1_001) / 2.0 - 250.0
        } else {
            (f64::from(self.next_u32()) / 4_294_967_296.0) * 500.0 - 250.0
        }
    }
}

/// Relative tolerance that absorbs one rounding step in the linear
/// interpolation without hiding a real deviation.
const EPS: f64 = 1e-9;

fn assert_close(actual: f64, expected: f64) {
    let scale = 1.0 + expected.abs().max(actual.abs());
    assert!(
        (actual - expected).abs() <= EPS * scale,
        "expected {expected}, observed {actual}"
    );
}

/// Returns a random-length sorted slice of random offsets.
fn sorted_sample(rng: &mut XorShift64) -> Vec<f64> {
    let len = 1 + rng.next_usize(64);
    let mut values: Vec<f64> = (0..len).map(|_| rng.next_offset()).collect();
    values.sort_by(f64::total_cmp);
    values
}

const QUANTILES: &[f64] = &[0.0, 10.0, 25.0, 50.0, 75.0, 95.0, 99.0, 100.0];
const STEPS: usize = 400;

/// Every quantile stays within the observed range, and the 0/100 quantiles
/// reproduce the exact extrema.
#[test]
fn quantiles_stay_bounded_and_hit_extrema() {
    let mut rng = XorShift64(0x5711_11C0_FFEE_0042);
    for _ in 0..STEPS {
        let sorted = sorted_sample(&mut rng);
        let min = sorted[0];
        let max = sorted[sorted.len() - 1];
        assert_close(percentile(&sorted, 0.0), min);
        assert_close(percentile(&sorted, 100.0), max);
        for &p in QUANTILES {
            let quantile = percentile(&sorted, p);
            assert!(
                quantile >= min - EPS && quantile <= max + EPS,
                "p={p} quantile={quantile} outside [{min}, {max}]"
            );
        }
    }
}

/// For a fixed sample, the reported quantile never decreases as the requested
/// percentile rises.
#[test]
fn percentile_is_monotone_in_the_requested_quantile() {
    let mut rng = XorShift64(0x6017_0A11_9000_00E5);
    for _ in 0..STEPS {
        let sorted = sorted_sample(&mut rng);
        let mut previous = percentile(&sorted, 0.0);
        for step in 1..=50 {
            let p = f64::from(step) * 2.0;
            let quantile = percentile(&sorted, p);
            assert!(
                quantile >= previous - EPS,
                "p={p} quantile={quantile} dropped below {previous}"
            );
            previous = quantile;
        }
    }
}

/// The median matches the linear-interpolation convention: the middle value
/// for odd lengths and the mean of the two middle values for even lengths.
#[test]
fn median_matches_the_linear_interpolation_convention() {
    let mut rng = XorShift64(0x0A11_4A55_2026_0042);
    for _ in 0..STEPS {
        let sorted = sorted_sample(&mut rng);
        let expected = if sorted.len() % 2 == 1 {
            sorted[sorted.len() / 2]
        } else {
            f64::midpoint(sorted[sorted.len() / 2 - 1], sorted[sorted.len() / 2])
        };
        assert_close(percentile(&sorted, 50.0), expected);
    }
}

/// `summarize_ms` reports the true extrema of unsorted input and keeps the
/// quantiles ordered inside those bounds.
#[test]
fn summarize_ms_reports_true_bounds_with_ordered_quantiles() {
    let mut rng = XorShift64(0xDEAD_BEEF_1234_5678);
    for _ in 0..STEPS {
        let len = 1 + rng.next_usize(64);
        let values: Vec<f64> = (0..len).map(|_| rng.next_offset()).collect();
        let summary = summarize_ms(&values).expect("values present");
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert_close(summary.min_ms, min);
        assert_close(summary.max_ms, max);
        for quantile in [summary.p50_ms, summary.p95_ms, summary.p99_ms] {
            assert!(
                quantile >= summary.min_ms - EPS && quantile <= summary.max_ms + EPS,
                "quantile={quantile} outside [{}, {}]",
                summary.min_ms,
                summary.max_ms
            );
        }
        assert!(
            summary.p50_ms <= summary.p95_ms + EPS && summary.p95_ms <= summary.p99_ms + EPS,
            "quantiles out of order: p50={} p95={} p99={}",
            summary.p50_ms,
            summary.p95_ms,
            summary.p99_ms
        );
    }
}

/// A single value or a run of identical values is reported unchanged for
/// every quantile.
#[test]
fn degenerate_inputs_repeat_the_observed_value() {
    let mut rng = XorShift64(0x00C0_FFEE_0000_0001);
    for _ in 0..STEPS {
        let value = rng.next_offset();
        let single = summarize_ms(&[value]).expect("one value");
        for quantile in [
            single.min_ms,
            single.p50_ms,
            single.p95_ms,
            single.p99_ms,
            single.max_ms,
        ] {
            assert_close(quantile, value);
        }

        let len = 2 + rng.next_usize(8);
        let constant = rng.next_offset();
        let repeated = summarize_ms(&vec![constant; len]).expect("constant values");
        for quantile in [
            repeated.min_ms,
            repeated.p50_ms,
            repeated.p95_ms,
            repeated.p99_ms,
            repeated.max_ms,
        ] {
            assert_close(quantile, constant);
        }
    }
}
