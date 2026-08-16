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

    /// Clears interpolation phase and returns the ratio to nominal.
    ///
    /// A stream boundary (epoch change or hard resynchronization) invalidates
    /// previously retained input frames; callers apply this before appending
    /// frames from the new epoch so no stale samples are interpolated across
    /// the boundary. Any applied ppm correction is dropped; the control domain
    /// re-publishes it after a reset.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.ratio = self.nominal_ratio;
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

/// Taps in the windowed-sinc kernel (16 past, center, and 16 future).
const POLYPHASE_TAPS: usize = 33;
/// Past and future taps around the center of the windowed-sinc kernel.
const POLYPHASE_HALF_TAPS: usize = 16;

/// Stateful windowed-sinc nominal-rate converter for one fixed interleaved
/// channel layout.
///
/// This is the reviewed higher-quality replacement candidate for
/// [`LinearResampler`]: every output sample is a normalized 33-tap
/// Blackman-Harris windowed-sinc convolution instead of a two-point linear
/// interpolation, which keeps the passband flat and attenuates out-of-band
/// images and aliasing. The kernel cutoff is the lower of the two Nyquist
/// rates, so upsampling and downsampling are both bandlimited.
///
/// It preserves the same bounded contract as [`LinearResampler`]:
/// construction precomputes the window, [`Self::process`] performs no
/// allocation, drift correction stays a separate control-domain concern, and
/// the caller retains the unconsumed input tail between calls.
pub struct PolyphaseResampler {
    channels: u16,
    nominal_ratio: f64,
    ratio: f64,
    phase: f64,
    /// Kernel cutoff as a fraction of the input sample rate.
    cutoff: f64,
    window: [f32; POLYPHASE_TAPS],
}

impl PolyphaseResampler {
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
        let cutoff = f64::from(input_sample_rate_hz.min(output_sample_rate_hz))
            / f64::from(input_sample_rate_hz);
        Ok(Self {
            channels,
            nominal_ratio,
            ratio: nominal_ratio,
            phase: 0.0,
            cutoff,
            window: blackman_harris_window(),
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
    /// and the occupancy controller's sign convention.
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

    /// Clears interpolation phase and returns the ratio to nominal.
    ///
    /// A stream boundary (epoch change or hard resynchronization) invalidates
    /// previously retained input frames; callers apply this before appending
    /// frames from the new epoch so no stale samples cross the boundary.
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.ratio = self.nominal_ratio;
    }

    /// Returns the input-frame capacity needed for a bounded output chunk.
    ///
    /// The kernel reserves [`POLYPHASE_HALF_TAPS`] future frames beyond the
    /// last output center plus the retained past window the caller keeps
    /// in-slice. The result is `None` only if the requested size overflows.
    #[must_use]
    pub fn input_frames_for_output(&self, output_frames: usize) -> Option<usize> {
        let last_center = frames_as_f64(output_frames.saturating_sub(1)) * self.ratio;
        phase_index(last_center).checked_add(2 * POLYPHASE_HALF_TAPS + 1)
    }

    /// Converts as many complete output frames as the supplied input permits.
    ///
    /// The output is initialized to silence before convolution. The caller
    /// must retire the returned number of input frames and prepend any retained
    /// input frames before the next call, exactly as with [`LinearResampler`].
    /// The kernel is normalized per output frame, so the DC gain is exactly one
    /// even while the past side is zero-padded during stream start-up.
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
            let center = self.phase;
            let center_floor = phase_index(center);
            // The kernel reaches `POLYPHASE_HALF_TAPS` future frames past the
            // center; stop when those are no longer inside the supplied input.
            if center_floor + POLYPHASE_HALF_TAPS >= input_frames {
                break;
            }

            let mut taps = [0.0f64; POLYPHASE_TAPS];
            let mut tap_sum = 0.0f64;
            for (offset, tap) in taps.iter_mut().enumerate() {
                // Unsigned kernel position; the real input index is this minus
                // the past half, and a negative result means zero-padded past.
                let kernel_position = center_floor + offset;
                let delta =
                    center - frames_as_f64(kernel_position) + frames_as_f64(POLYPHASE_HALF_TAPS);
                let scaled = delta * self.cutoff;
                let value = f64::from(self.window[offset]) * self.cutoff * sinc_f64(scaled);
                if kernel_position >= POLYPHASE_HALF_TAPS {
                    tap_sum += value;
                }
                *tap = value;
            }

            let output_frame = &mut output[produced * channels..(produced + 1) * channels];
            // The future-side guard keeps at least `POLYPHASE_HALF_TAPS + 1`
            // taps in range, so the normalizing sum is always positive.
            if tap_sum > 0.0 {
                for channel in 0..channels {
                    let mut sum = 0.0f64;
                    for (offset, tap) in taps.iter().enumerate() {
                        let kernel_position = center_floor + offset;
                        let input_index = kernel_position.checked_sub(POLYPHASE_HALF_TAPS);
                        let sample = match input_index {
                            Some(index) => f64::from(input[index * channels + channel]),
                            None => 0.0,
                        };
                        sum += sample * *tap;
                    }
                    output_frame[channel] = to_f32(sum / tap_sum);
                }
            }
            produced += 1;
            self.phase += self.ratio;
        }

        let consumed = phase_index(self.phase).saturating_sub(POLYPHASE_HALF_TAPS);
        self.phase -= frames_as_f64(consumed);
        Ok(ResampleOutcome {
            consumed_input_frames: consumed,
            produced_output_frames: produced,
        })
    }
}

/// Computes a normalized four-term Blackman-Harris window over the kernel.
#[allow(clippy::cast_precision_loss)]
fn blackman_harris_window() -> [f32; POLYPHASE_TAPS] {
    let mut window = [0.0f32; POLYPHASE_TAPS];
    let span = frames_as_f64(POLYPHASE_TAPS - 1);
    for (index, tap) in window.iter_mut().enumerate() {
        let angle = frames_as_f64(index) / span * std::f64::consts::TAU;
        *tap = to_f32(
            0.35875 - 0.48829 * angle.cos() + 0.14128 * (2.0 * angle).cos()
                - 0.01168 * (3.0 * angle).cos(),
        );
    }
    window
}

/// Evaluates the normalized sinc kernel: `sin(pi*x) / (pi*x)`, with `1` at zero.
fn sinc_f64(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        return 1.0;
    }
    let scaled = std::f64::consts::PI * x;
    scaled.sin() / scaled
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn to_f32(value: f64) -> f32 {
    value as f32
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

    #[test]
    fn ppm_adjustment_sign_matches_the_drift_consumption_convention() {
        let mut resampler = LinearResampler::new(48_000, 48_000, 1).unwrap();
        let output_frames = 100_000;

        let nominal = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(nominal, output_frames + 2);

        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let faster = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(faster, 100_202);
        assert!(faster > nominal);

        resampler
            .set_rate_adjustment_ppm(-MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let slower = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(slower, 99_802);
        assert!(slower < nominal);
    }

    #[test]
    fn input_capacity_helper_returns_none_only_on_overflow() {
        let mut resampler = LinearResampler::new(192_000, 8_000, 1).unwrap();
        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();

        assert_eq!(resampler.input_frames_for_output(1), Some(27));
        assert_eq!(resampler.input_frames_for_output(usize::MAX), None);
    }

    #[test]
    fn reset_clears_phase_and_drops_drift_correction() {
        let mut resampler = LinearResampler::new(48_000, 48_000, 1).unwrap();
        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let mut output = [f32::NAN; 3];
        let _ = resampler.process(&[0.0, 1.0, 2.0, 3.0, 4.0], &mut output);
        assert_eq!(resampler.input_frames_for_output(8), Some(11));

        resampler.reset();

        assert_eq!(resampler.input_frames_for_output(8), Some(10));
        let mut output = [f32::NAN; 3];
        let outcome = resampler.process(&[5.0, 6.0, 7.0, 8.0, 9.0], &mut output);
        assert_eq!(outcome.unwrap().produced_output_frames, 3);
        assert_samples(&output, &[5.0, 6.0, 7.0]);
    }

    #[test]
    fn polyphase_equal_rates_pass_through_in_steady_state() {
        let mut resampler = PolyphaseResampler::new(48_000, 48_000, 1).unwrap();
        let input: Vec<f32> = (0..64)
            .map(|index| to_f32(frames_as_f64(index) * 0.01))
            .collect();
        let mut output = [0.0f32; 64];
        let outcome = resampler.process(&input, &mut output).unwrap();

        // The kernel needs 16 future frames before the first output, and the
        // first 16 outputs zero-pad their past side; the steady-state middle
        // must pass the samples through unchanged.
        assert!(outcome.produced_output_frames > 32);
        let produced = outcome.produced_output_frames;
        for index in POLYPHASE_HALF_TAPS..produced {
            assert!(
                (output[index] - input[index]).abs() < 1e-4,
                "output {index} drifted from input"
            );
        }
    }

    #[test]
    fn polyphase_resampling_snr_exceeds_linear_by_a_wide_margin() {
        let input_rate = 48_000.0;
        let output_rate = 44_100.0;
        let ratio = input_rate / output_rate;
        let frequency_hz = 997.0;
        let frames = 12_000;
        let input = sine_wave(frequency_hz, input_rate, frames, 1);

        let mut poly = PolyphaseResampler::new(48_000, 44_100, 1).unwrap();
        let mut linear = LinearResampler::new(48_000, 44_100, 1).unwrap();
        let mut poly_output = vec![0.0f32; frames];
        let mut linear_output = vec![0.0f32; frames];
        let poly_result = poly.process(&input, &mut poly_output).unwrap();
        let linear_result = linear.process(&input, &mut linear_output).unwrap();

        let produced = poly_result
            .produced_output_frames
            .min(linear_result.produced_output_frames);
        let start = 64;
        let end = produced.saturating_sub(64);
        assert!(end > start);
        let expected = resample_reference(frequency_hz, input_rate, ratio, start, end);
        let poly_snr = snr_db(&poly_output[start..end], &expected);
        let linear_snr = snr_db(&linear_output[start..end], &expected);

        assert!(poly_snr > 100.0, "polyphase SNR was only {poly_snr:.1} dB");
        assert!(
            linear_snr < poly_snr - 20.0,
            "linear SNR {linear_snr:.1} dB was not far below polyphase {poly_snr:.1} dB"
        );
    }

    #[test]
    fn polyphase_upsampling_keeps_high_snr() {
        let input_rate = 44_100.0;
        let output_rate = 48_000.0;
        let ratio = input_rate / output_rate;
        let frequency_hz = 997.0;
        let frames = 12_000;
        let input = sine_wave(frequency_hz, input_rate, frames, 1);

        let mut resampler = PolyphaseResampler::new(44_100, 48_000, 1).unwrap();
        let mut output = vec![0.0f32; frames];
        let produced = resampler
            .process(&input, &mut output)
            .unwrap()
            .produced_output_frames;
        let start = 64;
        let end = produced.saturating_sub(64);
        assert!(end > start);
        let expected = resample_reference(frequency_hz, input_rate, ratio, start, end);
        let snr = snr_db(&output[start..end], &expected);

        assert!(snr > 100.0, "upsampling SNR was only {snr:.1} dB");
    }

    #[test]
    fn polyphase_passband_gain_stays_flat() {
        let frames = 12_000;
        for frequency_hz in [100.0, 1_000.0, 5_000.0] {
            let input = sine_wave(frequency_hz, 48_000.0, frames, 1);
            let mut resampler = PolyphaseResampler::new(48_000, 44_100, 1).unwrap();
            let mut output = vec![0.0f32; frames];
            let produced = resampler
                .process(&input, &mut output)
                .unwrap()
                .produced_output_frames;
            let start = 64;
            let end = produced.saturating_sub(64);
            assert!(end > start);
            let input_rms = rms(&input[start..end]);
            let output_rms = rms(&output[start..end]);
            let gain_db = 20.0 * (output_rms / input_rms).log10();
            assert!(
                gain_db.abs() < 0.1,
                "passband gain at {frequency_hz} Hz was {gain_db:.3} dB"
            );
        }
    }

    #[test]
    fn polyphase_attenuates_an_out_of_band_tone() {
        // The 8 kHz output rate has a 4 kHz Nyquist; an 8 kHz input tone is
        // two octaves above the downsampling cutoff and must not alias in band.
        let frames = 12_000;
        let input = sine_wave(8_000.0, 48_000.0, frames, 1);
        let mut resampler = PolyphaseResampler::new(48_000, 8_000, 1).unwrap();
        let mut output = vec![0.0f32; frames];
        let produced = resampler
            .process(&input, &mut output)
            .unwrap()
            .produced_output_frames;
        let start = 64;
        let end = produced.saturating_sub(64);
        assert!(end > start);
        let input_rms = rms(&input[start..end]);
        let output_rms = rms(&output[start..end]);
        let attenuation_db = 20.0 * (output_rms / input_rms).log10();
        assert!(
            attenuation_db < -20.0,
            "out-of-band attenuation was only {attenuation_db:.1} dB"
        );
    }

    #[test]
    fn polyphase_rejects_invalid_inputs_and_initializes_output() {
        assert!(matches!(
            PolyphaseResampler::new(MIN_RESAMPLE_RATE_HZ - 1, 48_000, 2),
            Err(ResamplerError::InvalidInputSampleRate)
        ));
        assert!(matches!(
            PolyphaseResampler::new(48_000, MAX_RESAMPLE_RATE_HZ + 1, 2),
            Err(ResamplerError::InvalidOutputSampleRate)
        ));
        assert!(matches!(
            PolyphaseResampler::new(48_000, 48_000, 0),
            Err(ResamplerError::InvalidChannelCount)
        ));

        let mut resampler = PolyphaseResampler::new(48_000, 48_000, 2).unwrap();
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

    #[test]
    fn polyphase_ppm_adjustment_sign_matches_the_drift_consumption_convention() {
        let mut resampler = PolyphaseResampler::new(48_000, 48_000, 1).unwrap();
        let output_frames = 100_000;

        let nominal = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(nominal, 100_032);

        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let faster = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(faster, 100_231);
        assert!(faster > nominal);

        resampler
            .set_rate_adjustment_ppm(-MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let slower = resampler.input_frames_for_output(output_frames).unwrap();
        assert_eq!(slower, 99_832);
        assert!(slower < nominal);
    }

    #[test]
    fn polyphase_reset_clears_phase_and_drops_drift_correction() {
        let mut resampler = PolyphaseResampler::new(48_000, 48_000, 1).unwrap();
        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();
        let mut output = [0.0f32; 64];
        let _ = resampler.process(&sine_wave(440.0, 48_000.0, 64, 1), &mut output);
        assert_eq!(resampler.input_frames_for_output(100_000), Some(100_231));

        resampler.reset();

        assert_eq!(resampler.input_frames_for_output(100_000), Some(100_032));
        let mut output = [0.0f32; 64];
        let outcome = resampler.process(&[1.0f32; 64], &mut output).unwrap();
        assert!(outcome.produced_output_frames > 32);
        let produced = outcome.produced_output_frames;
        assert!(
            output[POLYPHASE_HALF_TAPS..produced]
                .iter()
                .all(|sample| (sample - 1.0).abs() < 1e-3)
        );
    }

    #[test]
    fn polyphase_input_capacity_helper_returns_none_only_on_overflow() {
        let mut resampler = PolyphaseResampler::new(192_000, 8_000, 1).unwrap();
        resampler
            .set_rate_adjustment_ppm(MAX_DRIFT_ADJUSTMENT_PPM)
            .unwrap();

        assert_eq!(resampler.input_frames_for_output(1), Some(33));
        assert_eq!(resampler.input_frames_for_output(usize::MAX), None);
    }

    #[test]
    fn polyphase_consumed_and_produced_stay_within_bounds() {
        let mut resampler = PolyphaseResampler::new(44_100, 48_000, 2).unwrap();
        let input = sine_wave(997.0, 44_100.0, 512, 2);
        for input_frames in [0usize, 1, 16, 33, 64, 200, 512] {
            let input_len = input_frames * 2;
            let mut output = vec![f32::NAN; 1_024];
            let outcome = resampler.process(&input[..input_len], &mut output).unwrap();
            assert!(outcome.consumed_input_frames <= input_frames);
            assert!(outcome.produced_output_frames <= 512);
            assert!(output.iter().all(|sample| sample.is_finite()));
            if input_frames > 0 {
                resampler.reset();
            }
        }
    }

    fn sine_wave(frequency_hz: f64, sample_rate_hz: f64, frames: usize, channels: u16) -> Vec<f32> {
        let channels = usize::from(channels);
        let mut samples = Vec::with_capacity(frames * channels);
        for frame in 0..frames {
            let value = (std::f64::consts::TAU * frequency_hz * frames_as_f64(frame)
                / sample_rate_hz)
                .sin();
            for _ in 0..channels {
                samples.push(to_f32(value));
            }
        }
        samples
    }

    fn resample_reference(
        frequency_hz: f64,
        input_rate: f64,
        ratio: f64,
        start: usize,
        end: usize,
    ) -> Vec<f32> {
        (start..end)
            .map(|index| {
                let input_time = frames_as_f64(index) * ratio;
                to_f32((std::f64::consts::TAU * frequency_hz * input_time / input_rate).sin())
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum: f64 = samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum();
        (sum / frames_as_f64(samples.len())).sqrt()
    }

    fn snr_db(actual: &[f32], reference: &[f32]) -> f64 {
        let mut signal = 0.0f64;
        let mut noise = 0.0f64;
        for (actual, reference) in actual.iter().zip(reference) {
            let expected = f64::from(*reference);
            signal += expected * expected;
            let error = f64::from(*actual) - expected;
            noise += error * error;
        }
        if noise <= f64::MIN_POSITIVE {
            return f64::INFINITY;
        }
        10.0 * (signal / noise).log10()
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
