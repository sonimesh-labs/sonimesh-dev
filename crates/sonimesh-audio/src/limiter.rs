//! Allocation-free peak limiter for the realtime output chain.
//!
//! The limiter is the bounded-output stage of the Phase 2 mixing chain
//! (`per-stream gain -> fade/mute -> sum -> headroom -> optional limiter`). It
//! guarantees that no output sample exceeds a fixed ceiling, so a downstream
//! fixed-point conversion cannot clip, while remaining allocation-free,
//! lock-free, and `unsafe`-free inside the output callback.

use core::fmt;

use crate::channel_map::MAX_AUDIO_CHANNELS;

/// Default output ceiling: full scale.
pub const DEFAULT_CEILING: f32 = 1.0;

/// Default per-sample release coefficient (about a 10 ms time constant at
/// 48 kHz). Callers should tune it with
/// [`LimiterConfig::release_coefficient_for_seconds`] for their sample rate.
pub const DEFAULT_RELEASE_COEFFICIENT: f32 = 0.002;

/// Configuration for [`PeakLimiter`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterConfig {
    /// Output ceiling in `(0.0, 1.0]`. No output sample exceeds this value.
    pub ceiling: f32,
    /// Per-sample release coefficient in `(0.0, 1.0]`; `1.0` releases
    /// instantly.
    pub release_coefficient: f32,
}

impl LimiterConfig {
    /// Builds a configuration after validating both bounds.
    ///
    /// # Errors
    ///
    /// Returns a ceiling error when `ceiling` is not finite or is outside
    /// `(0.0, 1.0]`, or a release-coefficient error when the coefficient is
    /// not finite or is outside `(0.0, 1.0]`.
    pub const fn new(ceiling: f32, release_coefficient: f32) -> Result<Self, LimiterConfigError> {
        if !ceiling.is_finite() || ceiling <= 0.0 || ceiling > 1.0 {
            return Err(LimiterConfigError::InvalidCeiling);
        }
        if !release_coefficient.is_finite()
            || release_coefficient <= 0.0
            || release_coefficient > 1.0
        {
            return Err(LimiterConfigError::InvalidReleaseCoefficient);
        }
        Ok(Self {
            ceiling,
            release_coefficient,
        })
    }

    /// Per-sample release coefficient for a given release time and sample rate.
    ///
    /// The returned coefficient leaves about 37% of the gain deficit after
    /// `release_seconds`, so it is the standard exponential time constant. A
    /// non-positive time or sample rate yields `1.0` (instant release).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn release_coefficient_for_seconds(release_seconds: f32, sample_rate: u32) -> f32 {
        // Sample rates are small (tens of kHz), so the u32-to-f32 cast is exact.
        let denominator = release_seconds * (sample_rate as f32);
        if denominator.is_nan() || denominator <= 0.0 {
            return 1.0;
        }
        1.0 - (-1.0 / denominator).exp()
    }
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            ceiling: DEFAULT_CEILING,
            release_coefficient: DEFAULT_RELEASE_COEFFICIENT,
        }
    }
}

/// Failure while configuring a [`PeakLimiter`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimiterConfigError {
    /// The ceiling is not finite or is outside `(0.0, 1.0]`.
    InvalidCeiling,
    /// The release coefficient is not finite or is outside `(0.0, 1.0]`.
    InvalidReleaseCoefficient,
}

impl fmt::Display for LimiterConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidCeiling => "ceiling must be finite and in (0.0, 1.0]",
            Self::InvalidReleaseCoefficient => {
                "release coefficient must be finite and in (0.0, 1.0]"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for LimiterConfigError {}

/// Failure while applying an interleaved block through a [`PeakLimiter`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimiterError {
    /// The channel count is zero or exceeds [`MAX_AUDIO_CHANNELS`].
    InvalidChannelCount,
    /// The sample count is not divisible by the channel count.
    MisalignedFrames,
}

impl fmt::Display for LimiterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidChannelCount => {
                "channel count must be within the bounded audio channel range"
            }
            Self::MisalignedFrames => "sample count must be divisible by the channel count",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for LimiterError {}

/// Realtime-safe peak limiter with an exact output ceiling.
///
/// The limiter attacks sample-instantly, so it is a safety stage rather than a
/// lookahead musical limiter: the moment an input sample exceeds the ceiling,
/// the gain drops to bound that sample, and the output is clamped to the
/// ceiling. When the input returns below the ceiling the gain releases
/// smoothly toward unity, so sub-ceiling audio is transparent after full
/// release. Non-finite input fails closed to a bounded zero sample without
/// changing the limiter state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PeakLimiter {
    config: LimiterConfig,
    gain: f32,
}

impl PeakLimiter {
    /// Creates a limiter with the given configuration.
    #[must_use]
    pub const fn new(config: LimiterConfig) -> Self {
        Self { config, gain: 1.0 }
    }

    /// Limits one contiguous mono (or already decoupled) block in place.
    ///
    /// Each sample is limited independently; use
    /// [`PeakLimiter::apply_interleaved`] when channel balance must be
    /// preserved.
    pub fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            *sample = self.limit_sample(*sample);
        }
    }

    /// Limits one interleaved block in place, linking the gain across channels.
    ///
    /// The strongest reduction required by any channel in a frame is applied to
    /// the whole frame, which preserves the channel balance of the mixed bus.
    ///
    /// # Errors
    ///
    /// Returns [`LimiterError::InvalidChannelCount`] when `channels` is zero or
    /// exceeds [`MAX_AUDIO_CHANNELS`], or [`LimiterError::MisalignedFrames`]
    /// when the sample count is not divisible by `channels`. The block is left
    /// untouched in both cases.
    pub fn apply_interleaved(
        &mut self,
        samples: &mut [f32],
        channels: usize,
    ) -> Result<(), LimiterError> {
        if channels == 0 || channels > usize::from(MAX_AUDIO_CHANNELS) {
            return Err(LimiterError::InvalidChannelCount);
        }
        if samples.len() % channels != 0 {
            return Err(LimiterError::MisalignedFrames);
        }
        for frame in samples.chunks_exact_mut(channels) {
            let mut required: f32 = 1.0;
            for sample in frame.iter() {
                if sample.is_finite() {
                    let magnitude = sample.abs();
                    if magnitude > self.config.ceiling {
                        required = required.min(self.config.ceiling / magnitude);
                    }
                }
            }
            self.advance(required);
            for sample in frame.iter_mut() {
                *sample = self.scale(*sample);
            }
        }
        Ok(())
    }

    /// Current limiter gain, always in `(0.0, 1.0]`.
    #[must_use]
    pub const fn gain(&self) -> f32 {
        self.gain
    }

    /// Applies one sample through the limiter.
    fn limit_sample(&mut self, sample: f32) -> f32 {
        if !sample.is_finite() {
            return 0.0;
        }
        let magnitude = sample.abs();
        let required = if magnitude > self.config.ceiling {
            self.config.ceiling / magnitude
        } else {
            1.0
        };
        self.advance(required);
        self.scale(sample)
    }

    /// Advances the gain state toward `required`: instant attack when the
    /// input needs more reduction, smooth exponential release otherwise.
    ///
    /// The attack ratio `ceiling / magnitude` can underflow to zero for an
    /// extreme finite sample against a very low ceiling; flooring at the
    /// smallest positive subnormal keeps the documented `(0.0, 1.0]` gain bound
    /// while still reducing that sample to inaudible silence.
    fn advance(&mut self, required: f32) {
        if required < self.gain {
            self.gain = required.max(f32::from_bits(1));
        } else {
            self.gain += (1.0 - self.gain) * self.config.release_coefficient;
        }
    }

    /// Scales one sample by the current gain, clamped to the exact ceiling.
    ///
    /// Non-finite input fails closed to a bounded zero sample. The clamp makes
    /// the ceiling guarantee exact despite `f32` rounding in the multiply.
    fn scale(&self, sample: f32) -> f32 {
        if !sample.is_finite() {
            return 0.0;
        }
        (sample * self.gain).clamp(-self.config.ceiling, self.config.ceiling)
    }
}

impl Default for PeakLimiter {
    fn default() -> Self {
        Self::new(LimiterConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{LimiterConfig, LimiterConfigError, LimiterError, MAX_AUDIO_CHANNELS, PeakLimiter};

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn assert_zeroed(sample: f32) {
        assert!(
            sample.is_finite() && sample.to_bits() == 0.0_f32.to_bits(),
            "expected bounded zero, got {sample}"
        );
    }

    #[test]
    fn config_rejects_out_of_range_values() {
        for invalid in [0.0, -0.1, 1.01, 2.0, f32::NAN, f32::INFINITY] {
            assert!(
                matches!(
                    LimiterConfig::new(invalid, 0.1),
                    Err(LimiterConfigError::InvalidCeiling)
                ),
                "ceiling {invalid} should be rejected"
            );
        }
        for invalid in [0.0, -0.1, 1.01, 2.0, f32::NAN, f32::INFINITY] {
            assert!(
                matches!(
                    LimiterConfig::new(1.0, invalid),
                    Err(LimiterConfigError::InvalidReleaseCoefficient)
                ),
                "release coefficient {invalid} should be rejected"
            );
        }
    }

    #[test]
    fn defaults_are_valid_and_unity() {
        let config = LimiterConfig::default();
        assert_close(config.ceiling, 1.0);
        assert!(config.release_coefficient > 0.0 && config.release_coefficient <= 1.0);
        let limiter = PeakLimiter::default();
        assert_close(limiter.gain(), 1.0);
    }

    #[test]
    fn fresh_limiter_is_transparent_below_the_ceiling() {
        let mut limiter = PeakLimiter::default();
        let samples = [0.5, -0.25, 0.0, 1.0, -1.0];
        let mut output = samples;
        limiter.apply(&mut output);
        assert!(
            output
                .iter()
                .zip(samples)
                .all(|(left, right)| left.to_bits() == right.to_bits())
        );
        assert_close(limiter.gain(), 1.0);
    }

    #[test]
    fn a_single_peak_is_bounded_and_releases_smoothly() {
        let config = LimiterConfig::new(1.0, 0.5).unwrap();
        let mut limiter = PeakLimiter::new(config);
        let mut samples = [0.5, 3.0, 0.5, 0.5];
        limiter.apply(&mut samples);

        assert_close(samples[0], 0.5);
        assert_close(samples[1], 1.0);
        assert!(samples[1].abs() <= 1.0);
        // The following samples are still ducked by the reduced gain.
        assert!(samples[2] < 0.5 && samples[3] < 0.5);
        assert!(limiter.gain() < 1.0);

        // Sustained sub-ceiling input releases the gain back toward unity.
        let mut silence = [0.0; 32];
        limiter.apply(&mut silence);
        assert!(limiter.gain() > 0.9999);
        let mut quiet = [0.5; 4];
        let before = quiet;
        limiter.apply(&mut quiet);
        assert!(
            quiet
                .iter()
                .zip(before)
                .all(|(left, right)| left.to_bits() == right.to_bits())
        );
    }

    #[test]
    fn huge_peaks_are_clamped_to_the_exact_ceiling() {
        let config = LimiterConfig::new(0.9, 0.1).unwrap();

        let mut limiter = PeakLimiter::new(config);
        let mut positive = [1.0e30];
        limiter.apply(&mut positive);
        assert_close(positive[0], 0.9);
        assert!(positive[0].abs() <= 0.9);
        assert!(limiter.gain() < 1.0e-6);

        let mut limiter = PeakLimiter::new(config);
        let mut negative = [-1.0e30];
        limiter.apply(&mut negative);
        assert_close(negative[0], -0.9);
        assert!(negative[0].abs() <= 0.9);
        assert!(limiter.gain() < 1.0e-6);
    }

    #[test]
    fn non_finite_input_fails_closed_to_silence_without_state_change() {
        let mut limiter = PeakLimiter::default();
        let mut samples = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        limiter.apply(&mut samples);
        assert_zeroed(samples[0]);
        assert_zeroed(samples[1]);
        assert_zeroed(samples[2]);
        assert_close(samples[3], 0.5);
        assert_close(limiter.gain(), 1.0);
    }

    #[test]
    fn interleaved_frames_link_the_gain_across_channels() {
        let config = LimiterConfig::new(1.0, 0.2).unwrap();
        let mut limiter = PeakLimiter::new(config);
        let mut block = [0.5, 3.0];
        limiter.apply_interleaved(&mut block, 2).unwrap();

        // Both channels of the frame receive the same reduced gain.
        assert_close(block[0], 0.5 / 3.0);
        assert_close(block[1], 1.0);
        assert!(block[1].abs() <= 1.0);

        // A later quiet frame is still ducked but releases toward unity.
        let mut quiet = [0.5, 0.5];
        limiter.apply_interleaved(&mut quiet, 2).unwrap();
        assert!(quiet[0] < 0.5);
        assert_close(quiet[0], quiet[1]);
        assert!(limiter.gain() < 1.0);
    }

    #[test]
    fn interleaved_rejects_invalid_channels_and_alignment_without_mutation() {
        let mut limiter = PeakLimiter::default();
        let mut samples = [0.5, 0.5, 0.5];
        assert!(matches!(
            limiter.apply_interleaved(&mut samples, 0),
            Err(LimiterError::InvalidChannelCount)
        ));
        assert!(matches!(
            limiter.apply_interleaved(&mut samples, usize::from(MAX_AUDIO_CHANNELS) + 1),
            Err(LimiterError::InvalidChannelCount)
        ));
        assert!(matches!(
            limiter.apply_interleaved(&mut samples, 2),
            Err(LimiterError::MisalignedFrames)
        ));
        assert!(samples.iter().all(|sample| (sample - 0.5).abs() < 1e-6));
        assert_close(limiter.gain(), 1.0);
    }

    #[test]
    fn extreme_peaks_keep_the_gain_strictly_positive() {
        // `ceiling / magnitude` underflows to zero for a huge finite sample
        // against a very low ceiling; the documented (0.0, 1.0] gain bound and
        // the exact output ceiling must both survive.
        let config = LimiterConfig::new(1.27e-12, 0.5).unwrap();
        let mut limiter = PeakLimiter::new(config);
        let mut samples = [f32::MAX, 1.0];
        limiter.apply(&mut samples);

        assert!(limiter.gain() > 0.0 && limiter.gain() <= 1.0);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert!(samples.iter().all(|sample| sample.abs() <= config.ceiling));
    }

    #[test]
    fn release_coefficient_maps_seconds_and_sample_rate() {
        let coefficient = LimiterConfig::release_coefficient_for_seconds(0.01, 48000);
        let expected = 1.0 - (-1.0 / 480.0_f32).exp();
        assert_close(coefficient, expected);
        assert!(coefficient > 0.0 && coefficient < 1.0);

        assert_close(
            LimiterConfig::release_coefficient_for_seconds(0.0, 48000),
            1.0,
        );
        assert_close(
            LimiterConfig::release_coefficient_for_seconds(-1.0, 48000),
            1.0,
        );
        assert_close(LimiterConfig::release_coefficient_for_seconds(0.01, 0), 1.0);
    }
}
