use core::fmt;

/// Fixed internal sample rate for the first Opus feasibility path.
pub const OPUS_SAMPLE_RATE_HZ: u32 = 48_000;
/// Fixed channel count for the first Android playback-capture path.
pub const OPUS_CHANNELS: u16 = 2;
/// Shortest negotiated Opus packet duration admitted by the first feasibility path.
pub const MIN_OPUS_FRAME_DURATION_MS: u16 = 10;
/// Longest negotiated Opus packet duration admitted by the first feasibility path.
pub const MAX_OPUS_FRAME_DURATION_MS: u16 = 20;

/// Validated post-negotiation PCM shape for one first-path Opus packet.
///
/// This type is codec-neutral. It keeps the fixed 48 kHz stereo and 10/20 ms
/// policy explicit before a native decoder is selected, and it never allocates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpusFrameConfig {
    frame_duration_ms: u16,
}

impl OpusFrameConfig {
    /// Validates the initial Opus decode shape selected by the control plane.
    ///
    /// # Errors
    ///
    /// Returns an error when the negotiated stream differs from the first
    /// feasibility path policy: 48 kHz, stereo, and 10 or 20 milliseconds.
    pub const fn new(
        sample_rate_hz: u32,
        channels: u16,
        frame_duration_ms: u16,
    ) -> Result<Self, OpusFrameConfigError> {
        if sample_rate_hz != OPUS_SAMPLE_RATE_HZ {
            return Err(OpusFrameConfigError::UnsupportedSampleRate);
        }
        if channels != OPUS_CHANNELS {
            return Err(OpusFrameConfigError::UnsupportedChannels);
        }
        if !matches!(
            frame_duration_ms,
            MIN_OPUS_FRAME_DURATION_MS | MAX_OPUS_FRAME_DURATION_MS
        ) {
            return Err(OpusFrameConfigError::UnsupportedFrameDuration);
        }
        Ok(Self { frame_duration_ms })
    }

    /// Returns the negotiated packet duration in milliseconds.
    #[must_use]
    pub const fn frame_duration_ms(self) -> u16 {
        self.frame_duration_ms
    }

    /// Returns decoded samples per channel for one packet.
    #[must_use]
    pub const fn samples_per_channel(self) -> usize {
        (OPUS_SAMPLE_RATE_HZ as usize / 1_000) * self.frame_duration_ms as usize
    }

    /// Returns required interleaved Float32 PCM samples for one decoded packet.
    #[must_use]
    pub const fn interleaved_samples(self) -> usize {
        self.samples_per_channel() * OPUS_CHANNELS as usize
    }

    /// Validates a caller-owned PCM block capacity for this packet shape.
    ///
    /// A larger block is permitted so one stream can reserve 20 ms capacity
    /// before the 10/20 ms control-plane choice settles. The decoder must still
    /// report no more than the selected frame initialized sample count.
    ///
    /// # Errors
    ///
    /// Returns an error when the fixed output block cannot hold one selected
    /// decoded frame.
    pub const fn validate_pcm_block_capacity(
        self,
        pcm_samples_per_block: usize,
    ) -> Result<(), OpusFrameConfigError> {
        if pcm_samples_per_block < self.interleaved_samples() {
            return Err(OpusFrameConfigError::PcmBlockTooSmall);
        }
        Ok(())
    }
}

/// Rejection for an unsupported initial Opus stream shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpusFrameConfigError {
    /// The initial binding supports only 48 kHz decoding.
    UnsupportedSampleRate,
    /// The initial binding supports only stereo decode output.
    UnsupportedChannels,
    /// The initial binding supports only 10 ms or 20 ms packet durations.
    UnsupportedFrameDuration,
    /// The caller-owned fixed PCM output block cannot hold a selected packet.
    PcmBlockTooSmall,
}

impl fmt::Display for OpusFrameConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "unsupported initial Opus frame configuration: {self:?}"
        )
    }
}

impl std::error::Error for OpusFrameConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_two_planned_packet_shapes() {
        let ten_ms = OpusFrameConfig::new(48_000, 2, 10).unwrap();
        let twenty_ms = OpusFrameConfig::new(48_000, 2, 20).unwrap();

        assert_eq!(ten_ms.samples_per_channel(), 480);
        assert_eq!(ten_ms.interleaved_samples(), 960);
        assert_eq!(twenty_ms.samples_per_channel(), 960);
        assert_eq!(twenty_ms.interleaved_samples(), 1_920);
    }

    #[test]
    fn rejects_unselected_sample_rate_channel_and_duration() {
        assert_eq!(
            OpusFrameConfig::new(44_100, 2, 10),
            Err(OpusFrameConfigError::UnsupportedSampleRate)
        );
        assert_eq!(
            OpusFrameConfig::new(48_000, 1, 10),
            Err(OpusFrameConfigError::UnsupportedChannels)
        );
        assert_eq!(
            OpusFrameConfig::new(48_000, 2, 5),
            Err(OpusFrameConfigError::UnsupportedFrameDuration)
        );
    }

    #[test]
    fn permits_a_maximum_block_for_the_shorter_selected_duration() {
        let ten_ms = OpusFrameConfig::new(48_000, 2, 10).unwrap();
        let twenty_ms = OpusFrameConfig::new(48_000, 2, 20).unwrap();

        assert_eq!(ten_ms.validate_pcm_block_capacity(1_920), Ok(()));
        assert_eq!(
            twenty_ms.validate_pcm_block_capacity(1_919),
            Err(OpusFrameConfigError::PcmBlockTooSmall)
        );
    }
}
