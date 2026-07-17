use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{MAX_AUDIO_CHANNELS, MAX_RESAMPLE_RATE_HZ, MIN_RESAMPLE_RATE_HZ};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, RenderSourceFactory, RenderSourceFactoryError};

use crate::{ChannelMappedRenderSource, DriftAdjustmentHandle, SampleRateMappedRenderSource};

/// Negotiated interleaved PCM format supplied by the decoder-side source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourcePcmFormat {
    /// Input sample rate before nominal device conversion.
    pub sample_rate_hz: u32,
    /// Input interleaved channel count before device-layout mapping.
    pub channels: u16,
}

impl SourcePcmFormat {
    /// Validates one source format before output stream creation.
    ///
    /// # Errors
    ///
    /// Returns [`SourcePcmFormatError`] when the format is outside the shared
    /// platform-neutral resampling or channel bounds.
    pub fn new(sample_rate_hz: u32, channels: u16) -> Result<Self, SourcePcmFormatError> {
        if !(MIN_RESAMPLE_RATE_HZ..=MAX_RESAMPLE_RATE_HZ).contains(&sample_rate_hz) {
            return Err(SourcePcmFormatError::InvalidSampleRate);
        }
        if channels == 0 || channels > MAX_AUDIO_CHANNELS {
            return Err(SourcePcmFormatError::InvalidChannelCount);
        }
        Ok(Self {
            sample_rate_hz,
            channels,
        })
    }
}

/// Invalid decoder-side source PCM format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePcmFormatError {
    /// Source sample rate is outside the shared resampler bounds.
    InvalidSampleRate,
    /// Source channel count is outside the shared audio bounds.
    InvalidChannelCount,
}

impl fmt::Display for SourcePcmFormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSampleRate => formatter.write_str("invalid source PCM sample rate"),
            Self::InvalidChannelCount => formatter.write_str("invalid source PCM channel count"),
        }
    }
}

impl std::error::Error for SourcePcmFormatError {}

/// Control-domain factory that adapts one decoded source to a selected device format.
///
/// Each created source preserves the same drift adjustment handle, allowing a
/// controller to survive platform stream recreation without callback-side locks.
pub struct FormatAdaptedRenderSourceFactory<F> {
    source_factory: F,
    source_format: SourcePcmFormat,
    adjustment: DriftAdjustmentHandle,
}

impl<F> FormatAdaptedRenderSourceFactory<F> {
    /// Creates a format adapter factory with its own drift adjustment handle.
    pub fn new(
        source_factory: F,
        source_format: SourcePcmFormat,
        metrics: Option<Arc<StreamCounters>>,
    ) -> Self {
        Self::with_adjustment(
            source_factory,
            source_format,
            DriftAdjustmentHandle::new(metrics),
        )
    }

    /// Creates a factory that reuses a caller-owned drift adjustment handle.
    #[must_use]
    pub const fn with_adjustment(
        source_factory: F,
        source_format: SourcePcmFormat,
        adjustment: DriftAdjustmentHandle,
    ) -> Self {
        Self {
            source_factory,
            source_format,
            adjustment,
        }
    }

    /// Returns a clone of the persistent control-domain adjustment handle.
    #[must_use]
    pub fn adjustment_handle(&self) -> DriftAdjustmentHandle {
        self.adjustment.clone()
    }
}

impl<F> RenderSourceFactory for FormatAdaptedRenderSourceFactory<F>
where
    F: RenderSourceFactory,
{
    fn create(
        &mut self,
        config: &OutputConfig,
    ) -> Result<Box<dyn sonimesh_platform::RenderSource>, RenderSourceFactoryError> {
        config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)?;
        let source = self.source_factory.create(config)?;
        let rate_mapped = SampleRateMappedRenderSource::new_with_drift_handle(
            source,
            self.source_format.sample_rate_hz,
            config.sample_rate_hz,
            self.source_format.channels,
            config.maximum_callback_frames,
            &self.adjustment,
        )
        .map_err(|error| RenderSourceFactoryError::Build(error.to_string()))?;
        let channel_mapped = ChannelMappedRenderSource::new(
            Box::new(rate_mapped),
            self.source_format.channels,
            config.channels,
            config.maximum_callback_frames,
        )
        .map_err(|error| RenderSourceFactoryError::Build(error.to_string()))?;
        Ok(Box::new(channel_mapped))
    }
}
