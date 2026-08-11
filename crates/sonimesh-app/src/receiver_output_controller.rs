use core::fmt;

use sonimesh_audio::{FrameDecoder, RealtimePcmProducer};
use sonimesh_platform::{AudioOutput, OutputError, OutputHealth};

use crate::{
    GainControlHandle, OutputSelection, OutputSnapshot, OutputSnapshotSource,
    ReceiverPcmEndpointError, ReceiverPcmEndpointFactory, ReceiverStreamPipeline,
    UdpReceiverPipeline,
};

/// Control-domain boundary that installs a producer for a newly accepted output source.
pub trait ReceiverPcmProducerInstaller<const PCM_SAMPLES_PER_BLOCK: usize> {
    /// Retires the previous output queue and installs the producer paired with a new source.
    fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>);
}

impl<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>
    for ReceiverStreamPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>
where
    D: FrameDecoder,
{
    fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>) {
        ReceiverStreamPipeline::replace_pcm_producer(self, producer);
    }
}

impl<
    D,
    const JITTER_CAPACITY: usize,
    const MAX_ENCODED_BYTES: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
> ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>
    for UdpReceiverPipeline<D, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>
where
    D: FrameDecoder,
{
    fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>) {
        UdpReceiverPipeline::replace_pcm_producer(self, producer);
    }
}

/// Control-domain owner that recreates a receiver's paired PCM endpoint.
///
/// A platform callback source and the decoder-side producer are created as one
/// endpoint. The producer is installed into the receiver pipeline only after
/// the platform accepts its matching source.
pub struct ReceiverOutputController<O, const PCM_SAMPLES_PER_BLOCK: usize> {
    output: O,
    endpoint_factory: ReceiverPcmEndpointFactory<PCM_SAMPLES_PER_BLOCK>,
    selection: Option<OutputSelection>,
    stream_active: bool,
    fault_reported: bool,
}

impl<O, const PCM_SAMPLES_PER_BLOCK: usize> ReceiverOutputController<O, PCM_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    /// Creates a stopped receiver output controller.
    #[must_use]
    pub const fn new(
        output: O,
        endpoint_factory: ReceiverPcmEndpointFactory<PCM_SAMPLES_PER_BLOCK>,
    ) -> Self {
        Self {
            output,
            endpoint_factory,
            selection: None,
            stream_active: false,
            fault_reported: false,
        }
    }

    /// Starts a platform output and installs its paired producer on success.
    ///
    /// # Errors
    ///
    /// Returns before changing the receiver pipeline when a stream is already
    /// active, endpoint construction fails, or platform start fails.
    pub fn start<P>(
        &mut self,
        selection: OutputSelection,
        pipeline: &mut P,
    ) -> Result<(), ReceiverOutputLifecycleError>
    where
        P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    {
        if self.stream_active {
            return Err(ReceiverOutputLifecycleError::StreamAlreadyActive);
        }
        self.install(&selection, pipeline)?;
        self.selection = Some(selection);
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Stops a failed backend and returns its health once.
    #[must_use]
    pub fn monitor(&mut self) -> Option<OutputHealth> {
        if !self.stream_active || self.fault_reported {
            return None;
        }
        let health = self.output.health();
        if health == OutputHealth::Healthy {
            return None;
        }

        self.output.stop();
        self.endpoint_factory.clear_output_format();
        self.stream_active = false;
        self.fault_reported = true;
        Some(health)
    }

    /// Recreates the last selected output with a fresh paired PCM endpoint.
    ///
    /// # Errors
    ///
    /// Returns without changing the receiver pipeline when a stream is already
    /// active, endpoint construction fails, or platform start fails, so a later
    /// retry can reuse its existing producer.
    pub fn recover<P>(&mut self, pipeline: &mut P) -> Result<(), ReceiverOutputLifecycleError>
    where
        P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    {
        if self.stream_active {
            return Err(ReceiverOutputLifecycleError::StreamAlreadyActive);
        }
        let selection = self
            .selection
            .clone()
            .ok_or(ReceiverOutputLifecycleError::MissingSelection)?;
        self.install(&selection, pipeline)?;
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Stops the platform backend without mutating receiver lifecycle state.
    pub fn stop(&mut self) {
        self.output.stop();
        self.endpoint_factory.clear_output_format();
        self.stream_active = false;
        self.fault_reported = true;
    }

    /// Borrows the platform adapter for control-domain device operations.
    #[must_use]
    pub const fn output(&self) -> &O {
        &self.output
    }

    /// Mutably borrows the platform adapter outside the realtime callback.
    pub fn output_mut(&mut self) -> &mut O {
        &mut self.output
    }

    /// Returns a clone of the per-stream gain and mute handle.
    ///
    /// The handle is retained by the endpoint factory and shared with every
    /// created source, so a desktop owner can publish a local gain or mute
    /// choice that also survives output-device recovery.
    #[must_use]
    pub fn gain_handle(&self) -> GainControlHandle {
        self.endpoint_factory.gain_handle()
    }

    /// Returns a redacted low-rate output lifecycle snapshot.
    ///
    /// The projection mirrors [`OutputController::snapshot`], so either output
    /// owner can feed the desktop control snapshot without exposing device
    /// handles or audio material.
    #[must_use]
    pub fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot::from_health(
            self.selection.is_some(),
            self.stream_active,
            self.fault_reported,
            self.output.health(),
        )
    }

    fn install<P>(
        &mut self,
        selection: &OutputSelection,
        pipeline: &mut P,
    ) -> Result<(), ReceiverOutputLifecycleError>
    where
        P: ReceiverPcmProducerInstaller<PCM_SAMPLES_PER_BLOCK>,
    {
        let endpoint = self
            .endpoint_factory
            .create_deferred_metrics(&selection.config)
            .map_err(ReceiverOutputLifecycleError::Endpoint)?;
        let (producer, source) = endpoint.into_parts();
        self.output
            .start(selection.device.as_ref(), selection.config, source)
            .map_err(ReceiverOutputLifecycleError::Output)?;
        self.endpoint_factory
            .publish_output_format(&selection.config);
        pipeline.replace_pcm_producer(producer);
        Ok(())
    }
}

/// Failure while creating or recovering a paired receiver output endpoint.
#[derive(Debug)]
pub enum ReceiverOutputLifecycleError {
    /// Recovery has no successfully started output selection.
    MissingSelection,
    /// A start or recovery was requested while this controller already owns a stream.
    StreamAlreadyActive,
    /// The queue or source endpoint could not be created.
    Endpoint(ReceiverPcmEndpointError),
    /// The platform backend rejected the paired output source.
    Output(OutputError),
}

impl fmt::Display for ReceiverOutputLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSelection => {
                formatter.write_str("receiver output recovery has no selection")
            }
            Self::StreamAlreadyActive => {
                formatter.write_str("receiver output stream is already active")
            }
            Self::Endpoint(error) => {
                write!(formatter, "receiver PCM endpoint setup failed: {error}")
            }
            Self::Output(error) => write!(formatter, "receiver output backend failed: {error}"),
        }
    }
}

impl std::error::Error for ReceiverOutputLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingSelection | Self::StreamAlreadyActive => None,
            Self::Endpoint(error) => Some(error),
            Self::Output(error) => Some(error),
        }
    }
}

impl<O, const PCM_SAMPLES_PER_BLOCK: usize> OutputSnapshotSource
    for ReceiverOutputController<O, PCM_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    fn snapshot(&self) -> OutputSnapshot {
        ReceiverOutputController::snapshot(self)
    }
}
