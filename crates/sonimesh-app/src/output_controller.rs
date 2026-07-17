use core::fmt;

use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputError, OutputHealth, RenderSourceFactory,
    RenderSourceFactoryError,
};

/// Pinned or default output selection retained for control-domain recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputSelection {
    /// `None` follows the operating system's default output role.
    pub device: Option<OutputDeviceId>,
    /// Requested and bounded device format.
    pub config: OutputConfig,
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
    /// Returns [`OutputLifecycleError`] when stream creation fails. The caller
    /// commits `Streaming` through the control-session completion token.
    pub fn start(&mut self, selection: OutputSelection) -> Result<(), OutputLifecycleError> {
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
        self.stream_active = false;
        self.fault_reported = true;
        Some(health)
    }

    /// Recreates a failed stream using the last selection and a fresh source.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] when no previous selection exists or
    /// backend creation fails. The caller commits recovery separately.
    pub fn recover(&mut self) -> Result<(), OutputLifecycleError> {
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
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Stops the backend; lifecycle completion remains owned by the caller.
    pub fn stop(&mut self) {
        self.output.stop();
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
}

/// Output lifecycle or backend failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutputLifecycleError {
    /// No successful start has supplied a recovery selection.
    MissingSelection,
    /// Render-source construction failed before reaching the platform adapter.
    SourceFactory(RenderSourceFactoryError),
    /// Platform stream operation failed.
    Output(OutputError),
}

impl fmt::Display for OutputLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSelection => formatter.write_str("output recovery has no saved selection"),
            Self::SourceFactory(error) => {
                write!(formatter, "render source creation failed: {error}")
            }
            Self::Output(error) => write!(formatter, "output backend failed: {error}"),
        }
    }
}

impl std::error::Error for OutputLifecycleError {}
