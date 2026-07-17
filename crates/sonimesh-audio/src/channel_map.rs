use core::fmt;

/// Hard upper bound for a negotiated interleaved channel layout.
pub const MAX_AUDIO_CHANNELS: u16 = 8;

/// Failure while constructing or applying a bounded channel mapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelMappingError {
    /// The input layout has zero or more than [`MAX_AUDIO_CHANNELS`] channels.
    InvalidInputChannelCount,
    /// The output layout has zero or more than [`MAX_AUDIO_CHANNELS`] channels.
    InvalidOutputChannelCount,
    /// The input does not contain a whole number of interleaved frames.
    InputNotFrameAligned,
    /// The caller did not provide room for every mapped output sample.
    OutputTooSmall,
}

impl fmt::Display for ChannelMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidInputChannelCount => "invalid input channel count",
            Self::InvalidOutputChannelCount => "invalid output channel count",
            Self::InputNotFrameAligned => "input samples are not frame-aligned",
            Self::OutputTooSmall => "output buffer is too small for the mapped frames",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ChannelMappingError {}

/// Deterministic, allocation-free mapper for interleaved Float32 frames.
///
/// The mapper deliberately does not infer speaker positions. Mono is duplicated
/// to every output channel, a smaller input layout is copied and zero-extended,
/// and a larger input layout is folded into the output by averaging channels at
/// the same modulo position. A future speaker-layout decision can replace this
/// index-based policy without changing the realtime source boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelMapper {
    input_channels: u16,
    output_channels: u16,
}

impl ChannelMapper {
    /// Creates a mapper after validating both fixed channel-count bounds.
    ///
    /// # Errors
    ///
    /// Returns a channel-count error when either layout is outside the bounded
    /// `1..=8` range.
    pub const fn new(
        input_channels: u16,
        output_channels: u16,
    ) -> Result<Self, ChannelMappingError> {
        if input_channels == 0 || input_channels > MAX_AUDIO_CHANNELS {
            return Err(ChannelMappingError::InvalidInputChannelCount);
        }
        if output_channels == 0 || output_channels > MAX_AUDIO_CHANNELS {
            return Err(ChannelMappingError::InvalidOutputChannelCount);
        }
        Ok(Self {
            input_channels,
            output_channels,
        })
    }

    /// Returns the negotiated source channel count.
    #[must_use]
    pub const fn input_channels(self) -> u16 {
        self.input_channels
    }

    /// Returns the selected output channel count.
    #[must_use]
    pub const fn output_channels(self) -> u16 {
        self.output_channels
    }

    /// Maps complete interleaved input frames into caller-owned output storage.
    ///
    /// The whole output buffer is initialized to silence before mapping. The
    /// operation performs no allocation and is suitable for a preallocated
    /// realtime scratch boundary.
    ///
    /// # Errors
    ///
    /// Returns an alignment or capacity error without leaving uninitialized
    /// output samples.
    pub fn map(self, input: &[f32], output: &mut [f32]) -> Result<usize, ChannelMappingError> {
        output.fill(0.0);
        let input_channels = usize::from(self.input_channels);
        let output_channels = usize::from(self.output_channels);
        if input.len() % input_channels != 0 {
            return Err(ChannelMappingError::InputNotFrameAligned);
        }
        let frames = input.len() / input_channels;
        let output_samples = frames
            .checked_mul(output_channels)
            .ok_or(ChannelMappingError::OutputTooSmall)?;
        if output_samples > output.len() {
            return Err(ChannelMappingError::OutputTooSmall);
        }

        for frame in 0..frames {
            let input_frame = &input[frame * input_channels..(frame + 1) * input_channels];
            let output_frame = &mut output[frame * output_channels..(frame + 1) * output_channels];
            if input_channels == output_channels {
                output_frame.copy_from_slice(input_frame);
            } else if input_channels == 1 {
                output_frame.fill(input_frame[0]);
            } else if output_channels == 1 {
                output_frame[0] =
                    input_frame.iter().copied().sum::<f32>() / f32::from(self.input_channels);
            } else if input_channels < output_channels {
                output_frame[..input_channels].copy_from_slice(input_frame);
            } else {
                for (output_channel, sample) in output_frame.iter_mut().enumerate() {
                    let mut sum = 0.0;
                    let mut count = 0_u16;
                    let mut input_channel = output_channel;
                    while input_channel < input_channels {
                        sum += input_frame[input_channel];
                        count += 1;
                        input_channel += output_channels;
                    }
                    *sample = sum / f32::from(count);
                }
            }
        }
        Ok(output_samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_preserves_interleaved_frames() {
        let mapper = ChannelMapper::new(2, 2).unwrap();
        let input = [1.0, 2.0, 3.0, 4.0];
        let mut output = [f32::NAN; 4];

        assert_eq!(mapper.map(&input, &mut output), Ok(4));
        assert_samples(&output, &input);
    }

    #[test]
    fn mono_is_duplicated_to_every_output_channel() {
        let mapper = ChannelMapper::new(1, 3).unwrap();
        let input = [1.0, -0.5];
        let mut output = [f32::NAN; 6];

        assert_eq!(mapper.map(&input, &mut output), Ok(6));
        assert_samples(&output, &[1.0, 1.0, 1.0, -0.5, -0.5, -0.5]);
    }

    #[test]
    fn smaller_layout_is_zero_extended() {
        let mapper = ChannelMapper::new(2, 4).unwrap();
        let input = [1.0, 2.0];
        let mut output = [f32::NAN; 4];

        assert_eq!(mapper.map(&input, &mut output), Ok(4));
        assert_samples(&output, &[1.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    fn larger_layout_is_folded_by_modulo_average() {
        let mapper = ChannelMapper::new(4, 2).unwrap();
        let input = [1.0, 3.0, 5.0, 7.0];
        let mut output = [f32::NAN; 2];

        assert_eq!(mapper.map(&input, &mut output), Ok(2));
        assert_samples(&output, &[3.0, 5.0]);
    }

    #[test]
    fn rejects_invalid_layouts_and_unaligned_input() {
        assert_eq!(
            ChannelMapper::new(0, 2),
            Err(ChannelMappingError::InvalidInputChannelCount)
        );
        assert_eq!(
            ChannelMapper::new(2, MAX_AUDIO_CHANNELS + 1),
            Err(ChannelMappingError::InvalidOutputChannelCount)
        );

        let mapper = ChannelMapper::new(2, 2).unwrap();
        let mut output = [f32::NAN; 2];
        assert_eq!(
            mapper.map(&[1.0], &mut output),
            Err(ChannelMappingError::InputNotFrameAligned)
        );
        assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));
        assert_eq!(
            mapper.map(&[1.0, 2.0], &mut [f32::NAN; 1]),
            Err(ChannelMappingError::OutputTooSmall)
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
