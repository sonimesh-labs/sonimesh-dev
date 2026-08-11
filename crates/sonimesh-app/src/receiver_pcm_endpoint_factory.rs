use core::fmt;
use std::sync::Arc;

use sonimesh_audio::{RealtimePcmProducer, RealtimeQueueConfigError, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, RenderSource, RenderSourceFactoryError};

use crate::format_adapted_render_source_factory::adapt_render_source;
use crate::{DriftAdjustmentHandle, GainControlHandle, PcmRenderSource, SourcePcmFormat};

/// Paired PCM producer and callback source for one receiver output lifetime.
pub struct ReceiverPcmEndpoint<const SAMPLES_PER_BLOCK: usize> {
    producer: RealtimePcmProducer<SAMPLES_PER_BLOCK>,
    source: Box<dyn RenderSource>,
    gain: GainControlHandle,
}

impl<const SAMPLES_PER_BLOCK: usize> ReceiverPcmEndpoint<SAMPLES_PER_BLOCK> {
    /// Splits the paired endpoint for pipeline installation and platform start.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RealtimePcmProducer<SAMPLES_PER_BLOCK>,
        Box<dyn RenderSource>,
    ) {
        (self.producer, self.source)
    }

    /// Returns the gain and mute handle shared with this endpoint's source.
    #[must_use]
    pub fn gain_handle(&self) -> GainControlHandle {
        self.gain.clone()
    }
}

/// Failure while creating a fresh receiver PCM endpoint.
#[derive(Debug)]
pub enum ReceiverPcmEndpointError {
    /// Fixed PCM queue creation rejected the requested capacity.
    Queue(RealtimeQueueConfigError),
    /// Source format adaptation rejected the selected output configuration.
    Source(RenderSourceFactoryError),
}

impl fmt::Display for ReceiverPcmEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(formatter, "receiver PCM queue setup failed: {error}"),
            Self::Source(error) => write!(formatter, "receiver PCM source setup failed: {error}"),
        }
    }
}

impl std::error::Error for ReceiverPcmEndpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Source(error) => Some(error),
        }
    }
}

/// Control-domain factory for one receiver's queue and format-adapted source.
///
/// A consumer cannot be cloned into a recreated platform callback. This factory
/// instead creates a new bounded queue pair for each output lifetime and returns
/// its producer with the matching source so the stream owner can install both.
pub struct ReceiverPcmEndpointFactory<const SAMPLES_PER_BLOCK: usize> {
    queue_capacity: usize,
    source_format: SourcePcmFormat,
    counters: Arc<StreamCounters>,
    adjustment: DriftAdjustmentHandle,
    gain: GainControlHandle,
}

impl<const SAMPLES_PER_BLOCK: usize> ReceiverPcmEndpointFactory<SAMPLES_PER_BLOCK> {
    /// Creates a factory with a fresh persistent drift adjustment handle.
    #[must_use]
    pub fn new(
        queue_capacity: usize,
        source_format: SourcePcmFormat,
        counters: Arc<StreamCounters>,
    ) -> Self {
        Self::with_adjustment(
            queue_capacity,
            source_format,
            Arc::clone(&counters),
            DriftAdjustmentHandle::new(Some(counters)),
        )
    }

    /// Creates a factory that reuses one source-format drift handle.
    #[must_use]
    pub fn with_adjustment(
        queue_capacity: usize,
        source_format: SourcePcmFormat,
        counters: Arc<StreamCounters>,
        adjustment: DriftAdjustmentHandle,
    ) -> Self {
        Self {
            queue_capacity,
            source_format,
            counters,
            adjustment,
            gain: GainControlHandle::new(),
        }
    }

    /// Returns a clone of the drift adjustment shared by future sources.
    #[must_use]
    pub fn adjustment_handle(&self) -> DriftAdjustmentHandle {
        self.adjustment.clone()
    }

    /// Returns a clone of the gain and mute handle shared by future sources.
    ///
    /// The handle persists across output lifetimes, so a user's gain and mute
    /// choice survives output-device recreation.
    #[must_use]
    pub fn gain_handle(&self) -> GainControlHandle {
        self.gain.clone()
    }

    /// Creates one fresh queue pair and its format-adapted output source.
    ///
    /// # Errors
    ///
    /// Returns an error before platform start when queue allocation, output
    /// bounds, sample-rate conversion, or channel mapping cannot be prepared.
    pub fn create(
        &self,
        config: &OutputConfig,
    ) -> Result<ReceiverPcmEndpoint<SAMPLES_PER_BLOCK>, ReceiverPcmEndpointError> {
        self.create_with_adjustment(config, &self.adjustment)
    }

    /// Creates an endpoint without publishing its output format yet.
    ///
    /// The receiver output lifecycle controller calls
    /// [`Self::publish_output_format`] only after the platform accepts the
    /// matching callback source.
    pub(crate) fn create_deferred_metrics(
        &self,
        config: &OutputConfig,
    ) -> Result<ReceiverPcmEndpoint<SAMPLES_PER_BLOCK>, ReceiverPcmEndpointError> {
        let adjustment = self.adjustment.without_metrics();
        self.create_with_adjustment(config, &adjustment)
    }

    /// Publishes a platform-accepted output format to the stream diagnostics.
    pub(crate) fn publish_output_format(&self, config: &OutputConfig) {
        self.adjustment
            .publish_nominal_sample_rates(self.source_format.sample_rate_hz, config.sample_rate_hz);
    }

    /// Clears the format pair once its accepted callback source has stopped.
    pub(crate) fn clear_output_format(&self) {
        self.adjustment.clear_nominal_sample_rates();
    }

    fn create_with_adjustment(
        &self,
        config: &OutputConfig,
        adjustment: &DriftAdjustmentHandle,
    ) -> Result<ReceiverPcmEndpoint<SAMPLES_PER_BLOCK>, ReceiverPcmEndpointError> {
        config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)
            .map_err(ReceiverPcmEndpointError::Source)?;
        let (producer, consumer) =
            realtime_pcm_queue(self.queue_capacity).map_err(ReceiverPcmEndpointError::Queue)?;
        let source = Box::new(PcmRenderSource::new(
            consumer,
            Arc::clone(&self.counters),
            self.gain.clone(),
        ));
        let source = adapt_render_source(source, self.source_format, config, adjustment)
            .map_err(ReceiverPcmEndpointError::Source)?;
        Ok(ReceiverPcmEndpoint {
            producer,
            source,
            gain: self.gain.clone(),
        })
    }
}
