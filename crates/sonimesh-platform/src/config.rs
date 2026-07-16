use std::fmt;

/// Lowest output rate accepted by platform adapters.
pub const MIN_OUTPUT_SAMPLE_RATE_HZ: u32 = 8_000;
/// Highest output rate accepted by platform adapters.
pub const MAX_OUTPUT_SAMPLE_RATE_HZ: u32 = 192_000;
/// Highest interleaved output channel count.
pub const MAX_OUTPUT_CHANNELS: u16 = 8;
/// Smallest requested callback block.
pub const MIN_CALLBACK_FRAMES: u32 = 16;
/// Largest requested callback block.
pub const MAX_CALLBACK_FRAMES: u32 = 8_192;

/// Sample representation negotiated with the output device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleFormat {
    /// 32-bit floating point PCM.
    Float32,
    /// Signed 16-bit integer PCM.
    Signed16,
}

/// Concrete device format accepted outside the realtime callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputConfig {
    /// Device sample rate, which may differ from 48 kHz.
    pub sample_rate_hz: u32,
    /// Interleaved output channel count.
    pub channels: u16,
    /// Device sample representation.
    pub sample_format: SampleFormat,
    /// Requested callback size when the backend supports fixed buffers.
    /// `None` asks the backend to use its default cadence.
    pub requested_callback_frames: Option<u32>,
    /// Hard memory bound for any observed callback.
    pub maximum_callback_frames: u32,
}

impl OutputConfig {
    /// Validates all output resource bounds.
    ///
    /// # Errors
    ///
    /// Returns [`OutputConfigError`] for unsupported sample rate, channel count,
    /// callback frames, or an interleaved-size overflow.
    pub fn validate(self) -> Result<(), OutputConfigError> {
        if !(MIN_OUTPUT_SAMPLE_RATE_HZ..=MAX_OUTPUT_SAMPLE_RATE_HZ).contains(&self.sample_rate_hz) {
            return Err(OutputConfigError::InvalidSampleRate);
        }
        if !(1..=MAX_OUTPUT_CHANNELS).contains(&self.channels) {
            return Err(OutputConfigError::InvalidChannelCount);
        }
        if !(MIN_CALLBACK_FRAMES..=MAX_CALLBACK_FRAMES).contains(&self.maximum_callback_frames) {
            return Err(OutputConfigError::InvalidMaximumCallbackFrames);
        }
        if let Some(requested) = self.requested_callback_frames {
            if !(MIN_CALLBACK_FRAMES..=MAX_CALLBACK_FRAMES).contains(&requested) {
                return Err(OutputConfigError::InvalidRequestedCallbackFrames);
            }
            if requested > self.maximum_callback_frames {
                return Err(OutputConfigError::RequestedExceedsMaximum);
            }
        }
        self.interleaved_samples()?;
        Ok(())
    }

    /// Maximum interleaved samples allowed in one callback.
    ///
    /// # Errors
    ///
    /// Returns [`OutputConfigError::InterleavedSizeOverflow`] if multiplication
    /// cannot fit in `usize`.
    pub fn interleaved_samples(self) -> Result<usize, OutputConfigError> {
        usize::try_from(self.maximum_callback_frames)
            .ok()
            .and_then(|frames| frames.checked_mul(usize::from(self.channels)))
            .ok_or(OutputConfigError::InterleavedSizeOverflow)
    }
}

/// Invalid bounded output configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputConfigError {
    /// Sample rate is outside 8–192 kHz.
    InvalidSampleRate,
    /// Channel count is zero or greater than eight.
    InvalidChannelCount,
    /// Requested callback frames are outside 16–8192.
    InvalidRequestedCallbackFrames,
    /// Maximum callback frames are outside 16–8192.
    InvalidMaximumCallbackFrames,
    /// Fixed request exceeds the preallocated callback maximum.
    RequestedExceedsMaximum,
    /// Interleaved callback sample count overflowed `usize`.
    InterleavedSizeOverflow,
}

impl fmt::Display for OutputConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid bounded output configuration: {self:?}")
    }
}

impl std::error::Error for OutputConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> OutputConfig {
        OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 960,
        }
    }

    #[test]
    fn accepts_bounded_shared_mode_config() {
        let config = valid_config();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.interleaved_samples(), Ok(1_920));
    }

    #[test]
    fn rejects_each_unbounded_dimension() {
        let mut config = valid_config();
        config.sample_rate_hz = 384_000;
        assert_eq!(config.validate(), Err(OutputConfigError::InvalidSampleRate));

        config = valid_config();
        config.channels = 0;
        assert_eq!(
            config.validate(),
            Err(OutputConfigError::InvalidChannelCount)
        );

        config = valid_config();
        config.maximum_callback_frames = MAX_CALLBACK_FRAMES + 1;
        assert_eq!(
            config.validate(),
            Err(OutputConfigError::InvalidMaximumCallbackFrames)
        );

        config = valid_config();
        config.requested_callback_frames = Some(MAX_CALLBACK_FRAMES + 1);
        assert_eq!(
            config.validate(),
            Err(OutputConfigError::InvalidRequestedCallbackFrames)
        );

        config = valid_config();
        config.requested_callback_frames = Some(1_024);
        config.maximum_callback_frames = 512;
        assert_eq!(
            config.validate(),
            Err(OutputConfigError::RequestedExceedsMaximum)
        );
    }
}
