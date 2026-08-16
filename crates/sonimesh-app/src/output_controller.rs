use core::fmt;

use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputError, OutputHealth, RenderSourceFactory,
    RenderSourceFactoryError,
};

use crate::{OutputSnapshot, OutputSnapshotSource};

/// Pinned or default output selection retained for control-domain recovery.
#[derive(Clone, Eq, PartialEq)]
pub struct OutputSelection {
    /// `None` follows the operating system's default output role.
    pub device: Option<OutputDeviceId>,
    /// Requested and bounded device format.
    pub config: OutputConfig,
}

impl fmt::Debug for OutputSelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutputSelection")
            .field("device", &self.device)
            .field("config", &self.config)
            .finish()
    }
}

/// Desktop control-domain owner for output start, fault detection, and recreation.
pub struct OutputController<O, F> {
    output: O,
    source_factory: F,
    selection: Option<OutputSelection>,
    stream_active: bool,
    fault_reported: bool,
}

impl<O, F> OutputController<O, F>
where
    O: AudioOutput,
    F: RenderSourceFactory,
{
    /// Creates a stopped controller with no remembered output selection.
    #[must_use]
    pub const fn new(output: O, source_factory: F) -> Self {
        Self {
            output,
            source_factory,
            selection: None,
            stream_active: false,
            fault_reported: false,
        }
    }

    /// Starts a new output stream after the lifecycle owner prepared the action.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] when a stream is already active or
    /// creation fails. The caller commits `Streaming` through the control-session
    /// completion token.
    pub fn start(&mut self, selection: OutputSelection) -> Result<(), OutputLifecycleError> {
        if self.stream_active {
            return Err(OutputLifecycleError::StreamAlreadyActive);
        }
        selection
            .config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)
            .map_err(OutputLifecycleError::SourceFactory)?;
        let source = self
            .source_factory
            .create(&selection.config)
            .map_err(OutputLifecycleError::SourceFactory)?;
        self.output
            .start(selection.device.as_ref(), selection.config, source)
            .map_err(OutputLifecycleError::Output)?;
        self.source_factory.output_started(&selection.config);
        self.selection = Some(selection);
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Detects a platform fault and stops the stream outside the callback.
    ///
    /// Returns the observed fault once after stopping the backend.
    ///
    /// # Errors
    ///
    pub fn monitor(&mut self) -> Option<OutputHealth> {
        if !self.stream_active || self.fault_reported {
            return None;
        }
        let health = self.output.health();
        if health == OutputHealth::Healthy {
            return None;
        }

        self.output.stop();
        self.source_factory.output_stopped();
        self.stream_active = false;
        self.fault_reported = true;
        Some(health)
    }

    /// Recreates a failed stream using the last selection and a fresh source.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] when a stream is already active, no
    /// previous selection exists, or backend creation fails. The caller commits
    /// recovery separately.
    pub fn recover(&mut self) -> Result<(), OutputLifecycleError> {
        if self.stream_active {
            return Err(OutputLifecycleError::StreamAlreadyActive);
        }
        let selection = self
            .selection
            .clone()
            .ok_or(OutputLifecycleError::MissingSelection)?;
        selection
            .config
            .validate()
            .map_err(RenderSourceFactoryError::InvalidConfig)
            .map_err(OutputLifecycleError::SourceFactory)?;
        let source = self
            .source_factory
            .create(&selection.config)
            .map_err(OutputLifecycleError::SourceFactory)?;
        self.output
            .start(selection.device.as_ref(), selection.config, source)
            .map_err(OutputLifecycleError::Output)?;
        self.source_factory.output_started(&selection.config);
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Stops the backend; lifecycle completion remains owned by the caller.
    pub fn stop(&mut self) {
        self.output.stop();
        if self.stream_active {
            self.source_factory.output_stopped();
        }
        self.stream_active = false;
        self.fault_reported = true;
    }

    /// Returns a redacted low-rate output lifecycle snapshot.
    #[must_use]
    pub fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot::from_health(
            self.selection.is_some(),
            self.stream_active,
            self.fault_reported,
            self.output.health(),
        )
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
}

/// Output lifecycle or backend failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputLifecycleError {
    /// No successful start has supplied a recovery selection.
    MissingSelection,
    /// A start or recovery was requested while this controller already owns a stream.
    StreamAlreadyActive,
    /// Render-source construction failed before reaching the platform adapter.
    SourceFactory(RenderSourceFactoryError),
    /// Platform stream operation failed.
    Output(OutputError),
}

impl fmt::Display for OutputLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSelection => formatter.write_str("output recovery has no saved selection"),
            Self::StreamAlreadyActive => formatter.write_str("output stream is already active"),
            Self::SourceFactory(error) => {
                write!(formatter, "render source creation failed: {error}")
            }
            Self::Output(error) => write!(formatter, "output backend failed: {error}"),
        }
    }
}

impl std::error::Error for OutputLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingSelection | Self::StreamAlreadyActive => None,
            Self::SourceFactory(error) => Some(error),
            Self::Output(error) => Some(error),
        }
    }
}

impl<O, F> OutputSnapshotSource for OutputController<O, F>
where
    O: AudioOutput,
    F: RenderSourceFactory,
{
    fn snapshot(&self) -> OutputSnapshot {
        OutputController::snapshot(self)
    }
}

#[cfg(test)]
mod debug_tests {
    use sonimesh_platform::{OutputConfig, OutputDeviceId, SampleFormat};

    use super::OutputSelection;

    #[test]
    fn selection_debug_uses_the_redacted_device_identifier() {
        let selection = OutputSelection {
            device: Some(OutputDeviceId::new("persistent-output-id")),
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        };

        let debug = format!("{selection:?}");

        assert!(debug.contains("OutputDeviceId([REDACTED])"));
        assert!(!debug.contains("persistent-output-id"));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputConfigError, OutputDeviceId, OutputDeviceInfo,
        OutputError, OutputHealth, RenderSource, RenderSourceFactory, RenderSourceFactoryError,
        SampleFormat,
    };

    use super::{OutputController, OutputLifecycleError, OutputSelection};

    struct FakeOutput {
        started: bool,
        stopped: bool,
        last_device: Option<OutputDeviceId>,
        health: OutputHealth,
        fail_start: bool,
    }

    impl Default for FakeOutput {
        fn default() -> Self {
            Self {
                started: false,
                stopped: false,
                last_device: None,
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
            device: Option<&OutputDeviceId>,
            _config: OutputConfig,
            _source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            if self.fail_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.started = true;
            self.stopped = false;
            self.last_device = device.cloned();
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

    struct FakeSource;

    impl RenderSource for FakeSource {
        fn render(&mut self, interleaved_output: &mut [f32]) {
            interleaved_output.fill(0.0);
        }
    }

    #[derive(Default)]
    struct FactoryCounters {
        created: usize,
        started: usize,
        stopped: usize,
    }

    #[derive(Clone)]
    struct FakeSourceFactory {
        counters: Arc<Mutex<FactoryCounters>>,
        fail_create: bool,
    }

    impl Default for FakeSourceFactory {
        fn default() -> Self {
            Self {
                counters: Arc::new(Mutex::new(FactoryCounters::default())),
                fail_create: false,
            }
        }
    }

    impl RenderSourceFactory for FakeSourceFactory {
        fn create(
            &mut self,
            _config: &OutputConfig,
        ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError> {
            if self.fail_create {
                return Err(RenderSourceFactoryError::Build("test failure".to_owned()));
            }
            self.counters.lock().unwrap().created += 1;
            Ok(Box::new(FakeSource))
        }

        fn output_started(&mut self, _config: &OutputConfig) {
            self.counters.lock().unwrap().started += 1;
        }

        fn output_stopped(&mut self) {
            self.counters.lock().unwrap().stopped += 1;
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

    fn controller(output: FakeOutput) -> OutputController<FakeOutput, FakeSourceFactory> {
        OutputController::new(output, FakeSourceFactory::default())
    }

    #[test]
    fn new_starts_stopped_without_a_retained_selection() {
        let controller = controller(FakeOutput::default());

        let snapshot = controller.snapshot();
        assert!(!snapshot.selection_retained());
        assert!(!snapshot.stream_active());
        assert!(!snapshot.fault_reported());
        assert_eq!(snapshot.health(), OutputHealth::Healthy);
        assert!(!controller.output().started);
    }

    #[test]
    fn start_installs_the_source_and_remembers_the_selection() {
        let mut controller = controller(FakeOutput::default());

        controller.start(selection(None)).unwrap();

        assert!(controller.output().started);
        assert!(controller.output().last_device.is_none());
        let snapshot = controller.snapshot();
        assert!(snapshot.selection_retained());
        assert!(snapshot.stream_active());
        assert!(!snapshot.fault_reported());
    }

    #[test]
    fn start_passes_a_pinned_device_to_the_backend() {
        let mut controller = controller(FakeOutput::default());

        controller
            .start(selection(Some(OutputDeviceId::new("persistent-output-id"))))
            .unwrap();

        assert!(controller.output().last_device.is_some());
    }

    #[test]
    fn start_rejects_an_already_active_stream() {
        let mut controller = controller(FakeOutput::default());
        controller.start(selection(None)).unwrap();

        assert_eq!(
            controller.start(selection(None)),
            Err(OutputLifecycleError::StreamAlreadyActive)
        );
        assert!(controller.snapshot().stream_active());
    }

    #[test]
    fn invalid_selection_is_rejected_before_the_backend_or_factory_runs() {
        let mut controller = controller(FakeOutput::default());
        let mut invalid = selection(None);
        invalid.config.channels = 0;

        assert!(matches!(
            controller.start(invalid),
            Err(OutputLifecycleError::SourceFactory(
                RenderSourceFactoryError::InvalidConfig(OutputConfigError::InvalidChannelCount)
            ))
        ));
        assert!(!controller.output().started);
        assert!(!controller.snapshot().stream_active());
    }

    #[test]
    fn source_factory_failure_never_reaches_the_backend() {
        let factory = FakeSourceFactory {
            fail_create: true,
            ..Default::default()
        };
        let mut controller = OutputController::new(FakeOutput::default(), factory);

        assert!(matches!(
            controller.start(selection(None)),
            Err(OutputLifecycleError::SourceFactory(
                RenderSourceFactoryError::Build(_)
            ))
        ));
        assert!(!controller.output().started);
        assert!(!controller.snapshot().stream_active());
    }

    #[test]
    fn backend_failure_skips_the_output_started_hook() {
        let output = FakeOutput {
            fail_start: true,
            ..Default::default()
        };
        let factory = FakeSourceFactory::default();
        let counters = factory.counters.clone();
        let mut controller = OutputController::new(output, factory);

        assert_eq!(
            controller.start(selection(None)),
            Err(OutputLifecycleError::Output(OutputError::DeviceUnavailable))
        );
        // The factory created the source but the platform never accepted it,
        // so the started hook must not fire.
        assert_eq!(counters.lock().unwrap().created, 1);
        assert_eq!(counters.lock().unwrap().started, 0);
        assert!(!controller.output().started);
    }

    #[test]
    fn monitor_ignores_healthy_and_stopped_streams() {
        let mut controller = controller(FakeOutput::default());
        assert_eq!(controller.monitor(), None);

        controller.start(selection(None)).unwrap();
        assert_eq!(controller.monitor(), None);
        assert!(controller.snapshot().stream_active());
        assert!(!controller.snapshot().fault_reported());
    }

    #[test]
    fn monitor_reports_a_fault_once_and_stops_the_source() {
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let factory = FakeSourceFactory::default();
        let counters = factory.counters.clone();
        let mut controller = OutputController::new(output, factory);
        controller.start(selection(None)).unwrap();
        assert_eq!(counters.lock().unwrap().started, 1);

        assert_eq!(controller.monitor(), Some(OutputHealth::StreamError));
        assert!(controller.output().stopped);
        assert_eq!(counters.lock().unwrap().stopped, 1);
        assert!(!controller.snapshot().stream_active());
        assert!(controller.snapshot().fault_reported());
        // The same fault is reported only once.
        assert_eq!(controller.monitor(), None);
    }

    #[test]
    fn recover_requires_a_previous_selection() {
        let mut controller = controller(FakeOutput::default());

        assert_eq!(
            controller.recover(),
            Err(OutputLifecycleError::MissingSelection)
        );
    }

    #[test]
    fn recover_recreates_a_stream_after_a_fault() {
        let output = FakeOutput {
            health: OutputHealth::CallbackCapacityExceeded,
            ..Default::default()
        };
        let factory = FakeSourceFactory::default();
        let counters = factory.counters.clone();
        let mut controller = OutputController::new(output, factory);
        controller.start(selection(None)).unwrap();
        assert_eq!(
            controller.monitor(),
            Some(OutputHealth::CallbackCapacityExceeded)
        );

        controller.recover().unwrap();
        assert!(controller.output().started);
        assert_eq!(counters.lock().unwrap().created, 2);
        assert_eq!(counters.lock().unwrap().started, 2);
        assert!(controller.snapshot().stream_active());
        assert!(!controller.snapshot().fault_reported());
    }

    #[test]
    fn recover_failure_preserves_the_retired_state() {
        let output = FakeOutput {
            health: OutputHealth::StreamError,
            ..Default::default()
        };
        let mut controller = controller(output);
        controller.start(selection(None)).unwrap();
        controller.monitor();

        controller.output_mut().fail_start = true;
        assert_eq!(
            controller.recover(),
            Err(OutputLifecycleError::Output(OutputError::DeviceUnavailable))
        );
        assert!(!controller.output().started);
        assert!(!controller.snapshot().stream_active());
        assert!(controller.snapshot().fault_reported());
    }

    #[test]
    fn stop_retires_the_stream_and_marks_a_fault() {
        let factory = FakeSourceFactory::default();
        let counters = factory.counters.clone();
        let mut controller = OutputController::new(FakeOutput::default(), factory);
        controller.start(selection(None)).unwrap();

        controller.stop();
        assert!(controller.output().stopped);
        assert_eq!(counters.lock().unwrap().stopped, 1);
        let snapshot = controller.snapshot();
        assert!(snapshot.selection_retained());
        assert!(!snapshot.stream_active());
        assert!(snapshot.fault_reported());
    }

    #[test]
    fn stop_before_any_start_skips_the_output_stopped_hook() {
        let factory = FakeSourceFactory::default();
        let counters = factory.counters.clone();
        let mut controller = OutputController::new(FakeOutput::default(), factory);

        controller.stop();
        assert!(controller.output().stopped);
        // No active stream ever owned the source, so the factory is not told
        // that a source stopped.
        assert_eq!(counters.lock().unwrap().stopped, 0);
    }

    #[test]
    fn error_display_is_actionable_without_secrets() {
        assert_eq!(
            OutputLifecycleError::MissingSelection.to_string(),
            "output recovery has no saved selection"
        );
        assert_eq!(
            OutputLifecycleError::StreamAlreadyActive.to_string(),
            "output stream is already active"
        );
        assert_eq!(
            OutputLifecycleError::Output(OutputError::DeviceUnavailable).to_string(),
            "output backend failed: output device is unavailable"
        );
        assert_eq!(
            OutputLifecycleError::SourceFactory(RenderSourceFactoryError::Build("boom".to_owned()))
                .to_string(),
            "render source creation failed: render source setup failed: boom"
        );
    }

    #[test]
    fn wrapped_errors_retain_their_source() {
        let factory =
            OutputLifecycleError::SourceFactory(RenderSourceFactoryError::Build("boom".to_owned()));
        assert!(std::error::Error::source(&factory).is_some());

        let backend = OutputLifecycleError::Output(OutputError::DeviceUnavailable);
        assert!(std::error::Error::source(&backend).is_some());

        for leaf in [
            OutputLifecycleError::MissingSelection,
            OutputLifecycleError::StreamAlreadyActive,
        ] {
            assert!(
                std::error::Error::source(&leaf).is_none(),
                "{leaf:?} reports a source"
            );
        }
    }
}
