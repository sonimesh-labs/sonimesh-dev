use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{MAX_AUDIO_CHANNELS, MAX_RESAMPLE_RATE_HZ, MIN_RESAMPLE_RATE_HZ};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{
    OutputConfig, RenderSource, RenderSourceFactory, RenderSourceFactoryError,
};

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
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
        config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)?;
        let source = self.source_factory.create(config)?;
        // The platform may still reject this source. Publish its nominal output
        // format only from `output_started` after that boundary accepts it.
        let adjustment = self.adjustment.without_metrics();
        adapt_render_source(source, self.source_format, config, &adjustment)
    }

    fn output_started(&mut self, config: &OutputConfig) {
        self.adjustment
            .publish_nominal_sample_rates(self.source_format.sample_rate_hz, config.sample_rate_hz);
    }

    fn output_stopped(&mut self) {
        self.adjustment.clear_nominal_sample_rates();
    }
}

/// Installs nominal-rate conversion followed by device-layout mapping.
pub(crate) fn adapt_render_source(
    source: Box<dyn RenderSource>,
    source_format: SourcePcmFormat,
    config: &OutputConfig,
    adjustment: &DriftAdjustmentHandle,
) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
    config
        .validate()
        .map_err(RenderSourceFactoryError::InvalidConfig)?;
    let rate_mapped = SampleRateMappedRenderSource::new_with_drift_handle(
        source,
        source_format.sample_rate_hz,
        config.sample_rate_hz,
        source_format.channels,
        config.maximum_callback_frames,
        adjustment,
    )
    .map_err(|error| RenderSourceFactoryError::Build(error.to_string()))?;
    let channel_mapped = ChannelMappedRenderSource::new(
        Box::new(rate_mapped),
        source_format.channels,
        config.channels,
        config.maximum_callback_frames,
    )
    .map_err(|error| RenderSourceFactoryError::Build(error.to_string()))?;
    Ok(Box::new(channel_mapped))
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        OutputConfig, RenderSource, RenderSourceFactory, RenderSourceFactoryError, SampleFormat,
    };

    use super::FormatAdaptedRenderSourceFactory;
    use crate::SourcePcmFormat;

    struct FakeSource {
        value: f32,
    }

    impl RenderSource for FakeSource {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(self.value);
        }
    }

    struct CountingFactory {
        created: usize,
        value: f32,
    }

    impl RenderSourceFactory for CountingFactory {
        fn create(
            &mut self,
            _config: &OutputConfig,
        ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
            self.created += 1;
            Ok(Box::new(FakeSource { value: self.value }))
        }
    }

    fn config() -> OutputConfig {
        OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 1_024,
        }
    }

    fn factory(
        metrics: Option<Arc<StreamCounters>>,
    ) -> FormatAdaptedRenderSourceFactory<CountingFactory> {
        FormatAdaptedRenderSourceFactory::new(
            CountingFactory {
                created: 0,
                value: 0.25,
            },
            SourcePcmFormat::new(48_000, 1).expect("fixed test format is valid"),
            metrics,
        )
    }

    #[test]
    fn adjustment_handle_is_shared_across_clones() {
        let metrics = Arc::new(StreamCounters::default());
        let factory = factory(Some(Arc::clone(&metrics)));

        let handle = factory.adjustment_handle();
        assert_eq!(handle.current_ppm(), 0);
        handle.set_ppm(100).unwrap();
        assert_eq!(factory.adjustment_handle().current_ppm(), 100);
        assert_eq!(metrics.snapshot().drift_adjustment_ppm, 100);
    }

    #[test]
    fn create_rejects_an_invalid_output_config_before_the_inner_factory() {
        let mut factory = factory(None);
        let mut invalid = config();
        invalid.channels = 0;

        assert!(matches!(
            factory.create(&invalid),
            Err(RenderSourceFactoryError::InvalidConfig(_))
        ));
        assert_eq!(factory.source_factory.created, 0);
    }

    #[test]
    fn create_returns_a_format_adapted_source() {
        let mut factory = factory(None);
        let mut source = factory.create(&config()).unwrap();

        assert_eq!(factory.source_factory.created, 1);
        let mut output = [0.0; 256];
        source.render(&mut output);
        assert!(output.iter().all(|sample| (sample - 0.25).abs() < 1e-6));
    }

    #[test]
    fn output_lifecycle_publishes_and_clears_nominal_rates() {
        let metrics = Arc::new(StreamCounters::default());
        let mut factory = factory(Some(Arc::clone(&metrics)));

        factory.output_started(&config());
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.nominal_input_sample_rate_hz, 48_000);
        assert_eq!(snapshot.nominal_output_sample_rate_hz, 48_000);

        factory.output_stopped();
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.nominal_input_sample_rate_hz, 0);
        assert_eq!(snapshot.nominal_output_sample_rate_hz, 0);
    }
}
