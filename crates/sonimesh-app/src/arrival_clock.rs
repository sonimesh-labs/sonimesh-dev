//! Monotonic arrival-clock helper for the adaptive jitter controller.
//!
//! The [`ArrivalJitter`](sonimesh_audio::ArrivalJitter) estimator needs a
//! non-decreasing tick source with a known rate. This module provides a
//! ready-made process-monotonic source; callers may supply any `FnMut() -> u64`
//! clock instead, as long as it never moves backward.

/// Returns a `'static` monotonic tick source at `ticks_per_second`.
///
/// The returned closure measures elapsed time since construction using
/// [`std::time::Instant`], so it is monotonic within the process and needs no
/// shared epoch. It is safe to call from a packet-processing domain, never
/// from the output callback.
pub fn monotonic_tick_source(ticks_per_second: u64) -> impl FnMut() -> u64 {
    let started = std::time::Instant::now();
    let rate = u128::from(ticks_per_second);
    move || {
        let elapsed_nanos = started.elapsed().as_nanos();
        let ticks = elapsed_nanos.saturating_mul(rate) / 1_000_000_000;
        u64::try_from(ticks).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::monotonic_tick_source;

    #[test]
    fn ticks_are_non_decreasing_and_scale_with_the_rate() {
        let mut ticks = monotonic_tick_source(1_000);
        let first = ticks();
        for _ in 0..1_000 {
            let next = ticks();
            assert!(next >= first, "a monotonic clock must never move backward");
        }
        assert!(ticks() >= first);
    }

    #[test]
    fn zero_rate_produces_zero_ticks() {
        let mut ticks = monotonic_tick_source(0);
        assert_eq!(ticks(), 0);
        assert_eq!(ticks(), 0);
    }
}
