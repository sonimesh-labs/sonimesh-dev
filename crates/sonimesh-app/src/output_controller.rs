use core::fmt;

use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputError, OutputHealth, RenderSourceFactory,
};

use crate::{SessionEvent, SessionState, SessionTransitionError};

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
        }
    }

    /// Starts a new output stream from the trusted ready state.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] when session state is invalid or stream
    /// creation fails. The state enters `Streaming` only after backend start.
    pub fn start(
        &mut self,
        session: &mut SessionState,
        selection: OutputSelection,
    ) -> Result<(), OutputLifecycleError> {
        require_state(*session, SessionState::READY, SessionEvent::StartStream)?;
        self.output
            .start(
                selection.device.as_ref(),
                selection.config,
                self.source_factory.create(),
            )
            .map_err(OutputLifecycleError::Output)?;
        transition(session, SessionEvent::StartStream, &mut self.output)?;
        self.selection = Some(selection);
        Ok(())
    }

    /// Detects a platform fault and stops the stream outside the callback.
    ///
    /// Returns the observed fault once while changing `Streaming` to
    /// `OutputUnavailable`; healthy and already-stopped sessions return `None`.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] only if the session rejects the required
    /// output-loss transition.
    pub fn monitor(
        &mut self,
        session: &mut SessionState,
    ) -> Result<Option<OutputHealth>, OutputLifecycleError> {
        let health = self.output.health();
        if health == OutputHealth::Healthy || *session != SessionState::STREAMING {
            return Ok(None);
        }

        self.output.stop();
        transition(session, SessionEvent::OutputLost, &mut self.output)?;
        Ok(Some(health))
    }

    /// Recreates a failed stream using the last selection and a fresh source.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] when no previous selection exists, the
    /// session is not awaiting output, or backend creation fails. A failed
    /// recovery leaves the session in `OutputUnavailable` for retry or stop.
    pub fn recover(&mut self, session: &mut SessionState) -> Result<(), OutputLifecycleError> {
        require_state(
            *session,
            SessionState::OUTPUT_UNAVAILABLE,
            SessionEvent::OutputRestored,
        )?;
        let selection = self
            .selection
            .clone()
            .ok_or(OutputLifecycleError::MissingSelection)?;
        self.output
            .start(
                selection.device.as_ref(),
                selection.config,
                self.source_factory.create(),
            )
            .map_err(OutputLifecycleError::Output)?;
        transition(session, SessionEvent::OutputRestored, &mut self.output)
    }

    /// Stops the backend and applies the idempotent stream-stop transition.
    ///
    /// # Errors
    ///
    /// Returns [`OutputLifecycleError`] if the session transition is invalid.
    pub fn stop(&mut self, session: &mut SessionState) -> Result<(), OutputLifecycleError> {
        self.output.stop();
        transition(session, SessionEvent::StopStream, &mut self.output)
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
    /// Session event is not valid from the current state.
    Transition(SessionTransitionError),
    /// No successful start has supplied a recovery selection.
    MissingSelection,
    /// Platform stream operation failed.
    Output(OutputError),
}

impl fmt::Display for OutputLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transition(error) => {
                write!(formatter, "output lifecycle transition failed: {error}")
            }
            Self::MissingSelection => formatter.write_str("output recovery has no saved selection"),
            Self::Output(error) => write!(formatter, "output backend failed: {error}"),
        }
    }
}

impl std::error::Error for OutputLifecycleError {}

fn require_state(
    state: SessionState,
    expected: SessionState,
    event: SessionEvent,
) -> Result<(), OutputLifecycleError> {
    if state == expected {
        return Ok(());
    }
    Err(OutputLifecycleError::Transition(SessionTransitionError {
        state,
        event,
    }))
}

fn transition<O: AudioOutput>(
    session: &mut SessionState,
    event: SessionEvent,
    output: &mut O,
) -> Result<(), OutputLifecycleError> {
    if let Err(error) = session.apply(event) {
        output.stop();
        return Err(OutputLifecycleError::Transition(error));
    }
    Ok(())
}
