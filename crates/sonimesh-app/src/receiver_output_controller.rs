use core::fmt;

use sonimesh_audio::{FrameDecoder, RealtimePcmProducer};
use sonimesh_platform::{AudioOutput, OutputError, OutputHealth};
use sonimesh_protocol::MediaKeyContext;

use crate::{
    GainControlHandle, OutputSelection, OutputSnapshot, OutputSnapshotSource,
    ReceiverPcmEndpointError, ReceiverPcmEndpointFactory, ReceiverStreamPipeline,
    UdpReceiverPipeline,
};

/// Control-domain boundary that installs a producer for a newly accepted output source.
pub trait ReceiverPcmProducerInstaller<const PCM_SAMPLES_PER_BLOCK: usize> {
    /// Retires the previous output queue and installs the producer paired with a new source.
    fn replace_pcm_producer(&mut self, producer: RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>);

    /// Returns the configured media context, when the adapter can report it.
    fn media_context(&self) -> Option<MediaKeyContext> {
        None
    }

    /// Returns the configured Opus frame policy, if known.
    fn opus_frame_config(&self) -> Option<sonimesh_audio::OpusFrameConfig> {
        None
    }

    /// Checks whether the pipeline can prepare a fresh Opus frame policy.
    ///
    /// # Errors
    ///
    /// Returns an error if the reserved buffers cannot support the frame.
    fn validate_fresh_opus_frame(
        &self,
        frame: sonimesh_audio::OpusFrameConfig,
    ) -> Result<(), crate::FreshSessionInstallError> {
        frame
            .validate_pcm_block_capacity(PCM_SAMPLES_PER_BLOCK)
            .map_err(crate::FreshSessionInstallError::Frame)
    }
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

    fn media_context(&self) -> Option<MediaKeyContext> {
        Some(self.context())
    }

    fn opus_frame_config(&self) -> Option<sonimesh_audio::OpusFrameConfig> {
        ReceiverStreamPipeline::opus_frame_config(self)
    }

    fn validate_fresh_opus_frame(
        &self,
        frame: sonimesh_audio::OpusFrameConfig,
    ) -> Result<(), crate::FreshSessionInstallError> {
        ReceiverStreamPipeline::validate_fresh_opus_frame(self, frame)
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

    fn media_context(&self) -> Option<MediaKeyContext> {
        Some(self.context())
    }

    fn opus_frame_config(&self) -> Option<sonimesh_audio::OpusFrameConfig> {
        UdpReceiverPipeline::opus_frame_config(self)
    }

    fn validate_fresh_opus_frame(
        &self,
        frame: sonimesh_audio::OpusFrameConfig,
    ) -> Result<(), crate::FreshSessionInstallError> {
        UdpReceiverPipeline::validate_fresh_opus_frame(self, frame)
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

    /// Stops the platform backend without reporting a platform fault.
    ///
    /// Lifecycle completion remains owned by the caller. A deliberate stop is
    /// distinct from [`Self::monitor`], which records a non-healthy backend
    /// fault for the next control snapshot.
    pub fn stop(&mut self) {
        self.output.stop();
        self.endpoint_factory.clear_output_format();
        self.stream_active = false;
        self.fault_reported = false;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sonimesh_audio::RealtimePcmProducer;
    use sonimesh_diagnostics::StreamCounters;
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };

    use super::{ReceiverOutputController, ReceiverOutputLifecycleError};
    use crate::{
        OutputSelection, ReceiverPcmEndpointFactory, ReceiverPcmProducerInstaller, SourcePcmFormat,
    };

    const SAMPLES: usize = 256;

    struct FakeOutput {
        started: bool,
        stopped: bool,
        health: OutputHealth,
        fail_start: bool,
    }

    impl Default for FakeOutput {
        fn default() -> Self {
            Self {
                started: false,
                stopped: false,
                health: OutputHealth::Healthy,
                fail_start: false,
            }
        }
    }

    impl AudioOutput for FakeOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            if self.fail_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.started = true;
            self.stopped = false;
            Ok(())
        }

        fn stop(&mut self) {
            self.started = false;
            self.stopped = true;
        }

        fn health(&self) -> OutputHealth {
            self.health
        }
    }

    #[derive(Default)]
    struct FakePipeline {
        replaced: usize,
    }

    impl ReceiverPcmProducerInstaller<SAMPLES> for FakePipeline {
        fn replace_pcm_producer(&mut self, _producer: RealtimePcmProducer<SAMPLES>) {
            self.replaced += 1;
        }
    }

    fn selection(device: Option<OutputDeviceId>) -> OutputSelection {
        OutputSelection {
            device,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        }
    }

    fn endpoint_factory() -> ReceiverPcmEndpointFactory<SAMPLES> {
        ReceiverPcmEndpointFactory::new(
            8,
            SourcePcmFormat::new(48_000, 2).expect("fixed source format is valid"),
            Arc::new(StreamCounters::default()),
        )
    }

    fn controller(output: FakeOutput) -> ReceiverOutputController<FakeOutput, SAMPLES> {
        ReceiverOutputController::new(output, endpoint_factory())
    }

    #[test]
    fn start_installs_a_producer_only_after_backend_acceptance() {
        let mut pipeline = FakePipeline::default();
        let mut controller = controller(FakeOutput::default());

        controller.start(selection(None), &mut pipeline).unwrap();
        assert_eq!(pipeline.replaced, 1);
        assert!(controller.output().started);

        let snapshot = controller.snapshot();
        assert!(snapshot.selection_retained());
        assert!(snapshot.stream_active());
        assert!(!snapshot.fault_reported());
    }

    #[test]
    fn start_rejects_an_already_active_stream() {
        let mut pipeline = FakePipeline::default();
        let mut controller = controller(FakeOutput::default());
        controller.start(selection(None), &mut pipeline).unwrap();

        assert!(matches!(
            controller.start(selection(None), &mut pipeline),
            Err(ReceiverOutputLifecycleError::StreamAlreadyActive)
        ));
        assert_eq!(pipeline.replaced, 1);
    }

    #[test]
    fn invalid_selection_is_rejected_before_backend_start() {
        let mut pipeline = FakePipeline::default();
        let mut controller = controller(FakeOutput::default());
        let mut invalid = selection(None);
        invalid.config.channels = 0;

        assert!(matches!(
            controller.start(invalid, &mut pipeline),
            Err(ReceiverOutputLifecycleError::Endpoint(_))
        ));
        assert!(!controller.output().started);
        assert_eq!(pipeline.replaced, 0);
    }

    #[test]
    fn backend_failure_does_not_install_the_producer() {
        let mut pipeline = FakePipeline::default();
        let output = FakeOutput {
            fail_start: true,
            ..Default::default()
        };
        let mut controller = controller(output);

        assert!(matches!(
            controller.start(selection(None), &mut pipeline),
            Err(ReceiverOutputLifecycleError::Output(
                OutputError::DeviceUnavailable
            ))
        ));
        assert_eq!(pipeline.replaced, 0);
        assert!(!controller.snapshot().stream_active());
    }

    #[test]
    fn monitor_reports_a_fault_once() {
        let mut pipeline = FakePipeline::default();
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let mut controller = controller(output);
        controller.start(selection(None), &mut pipeline).unwrap();

        assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
        assert!(!controller.snapshot().stream_active());
        assert!(controller.snapshot().fault_reported());
        assert_eq!(controller.monitor(), None);
    }

    #[test]
    fn recover_requires_a_previous_selection() {
        let mut pipeline = FakePipeline::default();
        let mut controller = controller(FakeOutput::default());

        assert!(matches!(
            controller.recover(&mut pipeline),
            Err(ReceiverOutputLifecycleError::MissingSelection)
        ));
        assert_eq!(pipeline.replaced, 0);
    }

    #[test]
    fn recover_reinstalls_a_fresh_producer_after_a_fault() {
        let mut pipeline = FakePipeline::default();
        let output = FakeOutput {
            health: OutputHealth::CallbackCapacityExceeded,
            ..Default::default()
        };
        let mut controller = controller(output);
        controller.start(selection(None), &mut pipeline).unwrap();
        assert_eq!(
            controller.monitor(),
            Some(OutputHealth::CallbackCapacityExceeded)
        );

        controller.recover(&mut pipeline).unwrap();
        assert_eq!(pipeline.replaced, 2);
        assert!(controller.snapshot().stream_active());
        assert!(!controller.snapshot().fault_reported());
    }

    #[test]
    fn stop_keeps_the_selection_without_reporting_a_fault() {
        let mut pipeline = FakePipeline::default();
        let mut controller = controller(FakeOutput::default());
        controller.start(selection(None), &mut pipeline).unwrap();

        controller.stop();
        assert!(controller.output().stopped);
        let snapshot = controller.snapshot();
        assert!(snapshot.selection_retained());
        assert!(!snapshot.stream_active());
        assert!(!snapshot.fault_reported());
    }

    #[test]
    fn gain_handle_is_shared_and_survives_recovery() {
        let mut pipeline = FakePipeline::default();
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let mut controller = controller(output);
        controller.start(selection(None), &mut pipeline).unwrap();
        assert!(controller.gain_handle().set_gain(0.5).is_ok());
        assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
        controller.recover(&mut pipeline).unwrap();
        assert!(controller.gain_handle().set_gain(0.25).is_ok());
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            ReceiverOutputLifecycleError::MissingSelection.to_string(),
            "receiver output recovery has no selection"
        );
        assert_eq!(
            ReceiverOutputLifecycleError::StreamAlreadyActive.to_string(),
            "receiver output stream is already active"
        );
    }
}
