use core::fmt;

use crate::{MAX_AUDIO_CHANNELS, MAX_DRIFT_ADJUSTMENT_PPM};

/// Lowest sample rate accepted by the bounded linear resampler.
pub const MIN_RESAMPLE_RATE_HZ: u32 = 8_000;
/// Highest sample rate accepted by the bounded linear resampler.
pub const MAX_RESAMPLE_RATE_HZ: u32 = 192_000;

/// Failure while constructing or applying the bounded resampler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResamplerError {
    /// The input rate is outside the shared platform-neutral bound.
    InvalidInputSampleRate,
    /// The output rate is outside the shared platform-neutral bound.
    InvalidOutputSampleRate,
    /// The channel count is outside the shared audio bound.
    InvalidChannelCount,
    /// The requested drift correction exceeds the shared ppm bound.
    InvalidAdjustmentPpm,
    /// Input samples do not contain a whole number of interleaved frames.
    InputNotFrameAligned,
    /// Output samples do not contain a whole number of interleaved frames.
    OutputNotFrameAligned,
}

impl fmt::Display for ResamplerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidInputSampleRate => "invalid resampler input sample rate",
            Self::InvalidOutputSampleRate => "invalid resampler output sample rate",
            Self::InvalidChannelCount => "invalid resampler channel count",
            Self::InvalidAdjustmentPpm => "invalid resampler adjustment ppm",
            Self::InputNotFrameAligned => "resampler input is not frame-aligned",
            Self::OutputNotFrameAligned => "resampler output is not frame-aligned",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ResamplerError {}

/// Result of one bounded resampling step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResampleOutcome {
    /// Input frames that the caller may retire from its front buffer.
    pub consumed_input_frames: usize,
    /// Output frames initialized with interpolated samples.
    pub produced_output_frames: usize,
}

/// Stateful linear interpolator for one fixed interleaved channel layout.
///
/// This is a bounded nominal-rate conversion primitive. It deliberately does
/// not claim production-quality filtering or perform drift control; callers
/// can replace it with a reviewed higher-quality stage without changing the
/// source adapter contract.
pub struct LinearResampler {
    channels: u16,
    nominal_ratio: f64,
    ratio: f64,
    phase: f64,
}

impl LinearResampler {
    /// Creates a resampler for one input/output rate pair.
    ///
    /// # Errors
    ///
    /// Returns an error when either rate or the interleaved channel count is
    /// outside the fixed platform-neutral bounds.
    pub fn new(
        input_sample_rate_hz: u32,
        output_sample_rate_hz: u32,
        channels: u16,
    ) -> Result<Self, ResamplerError> {
        if !(MIN_RESAMPLE_RATE_HZ..=MAX_RESAMPLE_RATE_HZ).contains(&input_sample_rate_hz) {
            return Err(ResamplerError::InvalidInputSampleRate);
        }
        if !(MIN_RESAMPLE_RATE_HZ..=MAX_RESAMPLE_RATE_HZ).contains(&output_sample_rate_hz) {
            return Err(ResamplerError::InvalidOutputSampleRate);
        }
        if channels == 0 || channels > MAX_AUDIO_CHANNELS {
            return Err(ResamplerError::InvalidChannelCount);
        }
        let nominal_ratio = f64::from(input_sample_rate_hz) / f64::from(output_sample_rate_hz);
        Ok(Self {
            channels,
            nominal_ratio,
            ratio: nominal_ratio,
            phase: 0.0,
        })
    }

    /// Returns the fixed interleaved channel count.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// Applies a bounded ppm correction to the nominal input/output ratio.
    ///
    /// Positive values consume input faster, matching [`DriftDecision::Adjust`]
    /// and the occupancy controller's sign convention. This method is intended
    /// for the control boundary; the realtime adapter may only consume the
    /// resulting prevalidated value.
    ///
    /// # Errors
    ///
    /// Returns [`ResamplerError::InvalidAdjustmentPpm`] when the correction is
    /// outside the shared drift bound.
    pub fn set_rate_adjustment_ppm(&mut self, adjustment_ppm: i32) -> Result<(), ResamplerError> {
        if !(-MAX_DRIFT_ADJUSTMENT_PPM..=MAX_DRIFT_ADJUSTMENT_PPM).contains(&adjustment_ppm) {
            return Err(ResamplerError::InvalidAdjustmentPpm);
        }
        let multiplier = 1.0 + f64::from(adjustment_ppm) / 1_000_000.0;
        self.ratio = self.nominal_ratio * multiplier;
        Ok(())
    }

    /// Returns the input-frame capacity needed for a bounded output chunk.
    ///
    /// Two extra frames are reserved for the interpolation pair at the end of
    /// the chunk. The result is `None` only if the requested size overflows.
    #[must_use]
    pub fn input_frames_for_output(&self, output_frames: usize) -> Option<usize> {
        let estimated = frames_as_f64(output_frames) * self.ratio;
        phase_index(estimated.ceil()).checked_add(2)
    }

    /// Converts as many complete output frames as the supplied input permits.
    ///
    /// The output is initialized to silence before interpolation. The caller
    /// must retire the returned number of input frames and prepend any retained
    /// input frames before the next call.
    ///
    /// # Errors
    ///
    /// Returns an alignment error without leaving uninitialized output.
    pub fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
    ) -> Result<ResampleOutcome, ResamplerError> {
        output.fill(0.0);
        let channels = usize::from(self.channels);
        if input.len() % channels != 0 {
            return Err(ResamplerError::InputNotFrameAligned);
        }
        if output.len() % channels != 0 {
            return Err(ResamplerError::OutputNotFrameAligned);
        }

        let input_frames = input.len() / channels;
        let output_frames = output.len() / channels;
        let mut produced = 0;
        while produced < output_frames {
            let base = phase_index(self.phase);
            if base >= input_frames.saturating_sub(1) {
                break;
            }
            let input_frame = &input[base * channels..(base + 2) * channels];
            let output_frame = &mut output[produced * channels..(produced + 1) * channels];
            let fraction = fraction_as_f32(self.phase.fract());
            for channel in 0..channels {
                let first = input_frame[channel];
                let second = input_frame[channels + channel];
                output_frame[channel] = first + (second - first) * fraction;
            }
            produced += 1;
            self.phase += self.ratio;
        }

        let consumed = phase_index(self.phase).min(input_frames.saturating_sub(1));
        self.phase -= frames_as_f64(consumed);
        Ok(ResampleOutcome {
            consumed_input_frames: consumed,
            produced_output_frames: produced,
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn phase_index(phase: f64) -> usize {
    phase.floor() as usize
}

#[allow(clippy::cast_precision_loss)]
fn frames_as_f64(frames: usize) -> f64 {
    frames as f64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn fraction_as_f32(fraction: f64) -> f32 {
    fraction as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_rates_preserve_complete_frames() {
        let mut resampler = LinearResampler::new(48_000, 48_000, 1).unwrap();
        let mut output = [f32::NAN; 3];

        let outcome = resampler.process(&[1.0, 2.0, 3.0, 4.0, 5.0], &mut output);

        assert_eq!(
            outcome,
            Ok(ResampleOutcome {
                consumed_input_frames: 3,
                produced_output_frames: 3,
            })
        );
        assert_samples(&output, &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn upsampling_interpolates_between_adjacent_frames() {
        let mut resampler = LinearResampler::new(48_000, 96_000, 1).unwrap();
        let mut output = [f32::NAN; 4];

        let outcome = resampler.process(&[0.0, 1.0, 2.0, 3.0], &mut output);

        assert_eq!(outcome.unwrap().produced_output_frames, 4);
        assert_samples(&output, &[0.0, 0.5, 1.0, 1.5]);
    }

    #[test]
    fn downsampling_keeps_the_last_interpolation_frame_for_the_next_step() {
        let mut resampler = LinearResampler::new(96_000, 48_000, 1).unwrap();
        let mut output = [f32::NAN; 3];

        let outcome = resampler.process(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &mut output);

        assert_eq!(outcome.unwrap().consumed_input_frames, 5);
        assert_samples(&output, &[0.0, 2.0, 4.0]);
    }

    #[test]
    fn rejects_invalid_rates_channels_and_alignment() {
        assert!(matches!(
            LinearResampler::new(MIN_RESAMPLE_RATE_HZ - 1, 48_000, 2),
            Err(ResamplerError::InvalidInputSampleRate)
        ));
        assert!(matches!(
            LinearResampler::new(48_000, MAX_RESAMPLE_RATE_HZ + 1, 2),
            Err(ResamplerError::InvalidOutputSampleRate)
        ));
        assert!(matches!(
            LinearResampler::new(48_000, 48_000, 0),
            Err(ResamplerError::InvalidChannelCount)
        ));

        let mut resampler = LinearResampler::new(48_000, 48_000, 2).unwrap();
        assert_eq!(
            resampler.set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM + 1),
            Err(ResamplerError::InvalidAdjustmentPpm)
        );
        let mut output = [f32::NAN; 2];
        assert_eq!(
            resampler.process(&[1.0], &mut output),
            Err(ResamplerError::InputNotFrameAligned)
        );
        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
        assert_eq!(
            resampler.process(&[1.0, 2.0], &mut [f32::NAN; 1]),
            Err(ResamplerError::OutputNotFrameAligned)
        );
    }

    fn assert_samples(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (*actual - *expected).abs() < f32::EPSILON)
        );
    }
}
