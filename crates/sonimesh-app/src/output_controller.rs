use core::fmt;

use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputError, OutputHealth, RenderSourceFactory,
    RenderSourceFactoryError,
};

use crate::OutputSnapshot;

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
