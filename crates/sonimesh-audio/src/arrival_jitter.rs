//! Relative packet-arrival-jitter measurement.
//!
//! The estimator converts inter-arrival times into sender-sample units using
//! the negotiated sample rate, subtracts the sender's sample-timestamp delta,
//! and reports the absolute deviation in milliseconds. A constant one-way
//! delay contributes no jitter; only variation in arrival timing does, so the
//! measurement is independent of sender and receiver clock offset and needs no
//! shared clock with the sender.

use std::fmt;

/// Invalid relative arrival-jitter configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrivalJitterConfigError {
    /// The negotiated sample rate must be non-zero.
    ZeroSampleRate,
    /// The arrival clock rate must be non-zero.
    ZeroTickRate,
}

impl fmt::Display for ArrivalJitterConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSampleRate => formatter.write_str("sample rate must be non-zero"),
            Self::ZeroTickRate => formatter.write_str("arrival clock rate must be non-zero"),
        }
    }
}

impl std::error::Error for ArrivalJitterConfigError {}

/// Measures per-packet relative arrival jitter in milliseconds.
///
/// Call [`Self::observe`] once per authenticated packet with a non-decreasing
/// arrival tick and the packet's sender sample timestamp. The first call has no
/// baseline and returns `None`; later calls return the absolute inter-arrival
/// deviation converted to milliseconds. The estimator allocates nothing and is
/// safe to call from a packet-processing domain, never from the output
/// callback. The slow smoothing that turns these observations into a target
/// decision belongs to the [`AdaptiveJitterTarget`] controller, so this type
/// reports each raw deviation without state beyond the previous pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArrivalJitter {
    sample_rate_hz: u32,
    ticks_per_second: u64,
    previous_arrival_ticks: Option<u64>,
    previous_sample_timestamp: Option<u64>,
}

impl ArrivalJitter {
    /// Creates an estimator for one stream's negotiated sample rate.
    ///
    /// `ticks_per_second` is the rate of the caller's monotonic arrival clock,
    /// for example `1_000_000_000` for a nanosecond clock.
    ///
    /// # Errors
    ///
    /// Returns [`ArrivalJitterConfigError`] when either rate is zero.
    pub fn new(
        sample_rate_hz: u32,
        ticks_per_second: u64,
    ) -> Result<Self, ArrivalJitterConfigError> {
        if sample_rate_hz == 0 {
            return Err(ArrivalJitterConfigError::ZeroSampleRate);
        }
        if ticks_per_second == 0 {
            return Err(ArrivalJitterConfigError::ZeroTickRate);
        }
        Ok(Self {
            sample_rate_hz,
            ticks_per_second,
            previous_arrival_ticks: None,
            previous_sample_timestamp: None,
        })
    }

    /// Feeds one arrival observation and returns the jitter deviation in ms.
    ///
    /// Returns `None` on the first observation or when the arrival clock moves
    /// backward, so the caller can skip those packets without corrupting the
    /// baseline.
    #[must_use]
    pub fn observe(&mut self, arrival_ticks: u64, sample_timestamp: u64) -> Option<u32> {
        let (Some(previous_arrival_ticks), Some(previous_sample_timestamp)) =
            (self.previous_arrival_ticks, self.previous_sample_timestamp)
        else {
            self.previous_arrival_ticks = Some(arrival_ticks);
            self.previous_sample_timestamp = Some(sample_timestamp);
            return None;
        };
        if arrival_ticks < previous_arrival_ticks {
            return None;
        }
        self.previous_arrival_ticks = Some(arrival_ticks);
        self.previous_sample_timestamp = Some(sample_timestamp);
        let arrival_delta_ticks = u128::from(arrival_ticks - previous_arrival_ticks);
        let arrival_delta_samples = arrival_delta_ticks
            .saturating_mul(u128::from(self.sample_rate_hz))
            / u128::from(self.ticks_per_second);
        let sample_delta = i128::from(sample_timestamp) - i128::from(previous_sample_timestamp);
        let arrival_delta_samples = i128::try_from(arrival_delta_samples).unwrap_or(i128::MAX);
        let deviation_samples = arrival_delta_samples.abs_diff(sample_delta);
        let deviation_ms = deviation_samples
            .saturating_mul(1_000)
            .div_ceil(u128::from(self.sample_rate_hz));
        Some(u32::try_from(deviation_ms).unwrap_or(u32::MAX))
    }

    /// Clears the baseline so the next observation starts a fresh estimate.
    pub fn reset(&mut self) {
        self.previous_arrival_ticks = None;
        self.previous_sample_timestamp = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{ArrivalJitter, ArrivalJitterConfigError};

    const SAMPLE_RATE_HZ: u32 = 48_000;

    #[test]
    fn first_observation_has_no_baseline() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        assert_eq!(jitter.observe(10, 480), Some(0));
    }

    #[test]
    fn perfect_cadence_reports_zero_jitter() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        for frame in 1_u32..8 {
            let ticks = u64::from(frame) * 10;
            let samples = u64::from(frame) * 480;
            assert_eq!(jitter.observe(ticks, samples), Some(0));
        }
    }

    #[test]
    fn constant_one_way_delay_is_not_jitter() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        // A fixed 200 ms network offset disappears from the relative delta.
        assert_eq!(jitter.observe(200, 0), None);
        assert_eq!(jitter.observe(210, 480), Some(0));
        assert_eq!(jitter.observe(220, 960), Some(0));
    }

    #[test]
    fn arrival_variance_is_reported_in_milliseconds() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        assert_eq!(jitter.observe(10, 480), Some(0));
        // The third frame arrives 20 ms late: one 480-sample frame of deviation.
        assert_eq!(jitter.observe(40, 960), Some(20));
    }

    #[test]
    fn reordered_timestamps_count_as_jitter() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        assert_eq!(jitter.observe(10, 480), Some(0));
        // An older timestamp arriving later doubles the sample deviation.
        assert_eq!(jitter.observe(20, 0), Some(20));
    }

    #[test]
    fn backward_clock_observations_are_skipped_without_corruption() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        assert_eq!(jitter.observe(10, 480), Some(0));
        assert_eq!(jitter.observe(5, 960), None);
        // The glitched packet is ignored: the next measurement still uses the
        // last valid baseline, so only the 5 ms arrival slip remains.
        assert_eq!(jitter.observe(15, 960), Some(5));
    }

    #[test]
    fn reset_starts_a_fresh_baseline() {
        let mut jitter = ArrivalJitter::new(SAMPLE_RATE_HZ, 1_000).unwrap();
        assert_eq!(jitter.observe(0, 0), None);
        assert_eq!(jitter.observe(10, 480), Some(0));
        jitter.reset();
        assert_eq!(jitter.observe(100, 4800), None);
        assert_eq!(jitter.observe(110, 5280), Some(0));
    }

    #[test]
    fn rejects_zero_rates() {
        assert_eq!(
            ArrivalJitter::new(0, 1_000),
            Err(ArrivalJitterConfigError::ZeroSampleRate)
        );
        assert_eq!(
            ArrivalJitter::new(SAMPLE_RATE_HZ, 0),
            Err(ArrivalJitterConfigError::ZeroTickRate)
        );
    }
}
