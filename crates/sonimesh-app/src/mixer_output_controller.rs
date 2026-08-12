//! Control-domain output-lifetime owner for the Phase 2 mixer path.
//!
//! `MixerOutputController` mirrors [`ReceiverOutputController`] but for the
//! fixed-slot mixer: it owns the platform [`AudioOutput`] and creates one
//! [`MixerRenderSource`] per output lifetime. The source's membership handle is
//! cloned before the platform consumes the source and is exposed to the
//! control-domain stream session only while this controller owns an active
//! stream, so membership commands can never be published to a retired mixer.
//! Stopping or faulting the backend retires the handle; every start or recovery
//! installs a fresh handle paired with the new mixer source.

use core::fmt;

use sonimesh_audio::MixerMembershipHandle;
use sonimesh_platform::{AudioOutput, OutputError, OutputHealth};

use crate::{
    MixerRenderSource, MixerRenderSourceError, OutputSelection, OutputSnapshot,
    OutputSnapshotSource, SourcePcmFormat,
};

/// Control-domain owner that recreates a mixer-backed output per lifetime.
///
/// The mixer render source and its slot queues are created fresh for every
/// platform output lifetime. `start` and `recover` hand the new source to the
/// platform and keep a clone of its [`MixerMembershipHandle`]; `monitor` and
/// `stop` retire that handle so a stream session re-admits its streams against
/// the next lifetime's empty slot table.
pub struct MixerOutputController<O, const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize> {
    output: O,
    selection: Option<OutputSelection>,
    stream_active: bool,
    fault_reported: bool,
    membership: Option<MixerMembershipHandle<PCM_SAMPLES_PER_BLOCK>>,
    output_format: Option<SourcePcmFormat>,
}

impl<O, const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize>
    MixerOutputController<O, SLOTS, PCM_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    /// Creates a stopped controller with no remembered output selection.
    #[must_use]
    pub const fn new(output: O) -> Self {
        Self {
            output,
            selection: None,
            stream_active: false,
            fault_reported: false,
            membership: None,
            output_format: None,
        }
    }

    /// Starts a new mixer-backed output stream after the lifecycle owner
    /// prepared the action.
    ///
    /// # Errors
    ///
    /// Returns [`MixerOutputLifecycleError`] when a stream is already active or
    /// source or backend creation fails.
    pub fn start(&mut self, selection: OutputSelection) -> Result<(), MixerOutputLifecycleError> {
        if self.stream_active {
            return Err(MixerOutputLifecycleError::StreamAlreadyActive);
        }
        self.install(&selection)?;
        self.selection = Some(selection);
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Detects a platform fault and stops the stream outside the callback.
    ///
    /// Returns the observed fault once and retires the membership handle so
    /// admission commands cannot reach the stopped mixer.
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
        self.membership = None;
        self.output_format = None;
        self.stream_active = false;
        self.fault_reported = true;
        Some(health)
    }

    /// Recreates a failed mixer-backed stream using the last selection.
    ///
    /// The replacement source owns an empty slot table; the stream session
    /// re-admits its active streams against the returned membership handle.
    ///
    /// # Errors
    ///
    /// Returns [`MixerOutputLifecycleError`] when a stream is already active,
    /// no previous selection exists, or source or backend creation fails.
    pub fn recover(&mut self) -> Result<(), MixerOutputLifecycleError> {
        if self.stream_active {
            return Err(MixerOutputLifecycleError::StreamAlreadyActive);
        }
        let selection = self
            .selection
            .clone()
            .ok_or(MixerOutputLifecycleError::MissingSelection)?;
        self.install(&selection)?;
        self.stream_active = true;
        self.fault_reported = false;
        Ok(())
    }

    /// Stops the backend and retires the membership handle.
    ///
    /// Lifecycle completion remains owned by the caller.
    pub fn stop(&mut self) {
        self.output.stop();
        self.membership = None;
        self.output_format = None;
        self.stream_active = false;
        self.fault_reported = true;
    }

    /// Returns the membership handle sharing the current mixer source.
    ///
    /// `None` while no stream is active; the handle is replaced on every
    /// start or recovery, so callers must re-admit streams after recreation.
    #[must_use]
    pub fn membership(&self) -> Option<MixerMembershipHandle<PCM_SAMPLES_PER_BLOCK>> {
        self.membership.clone()
    }

    /// Returns the output-format PCM layout of the current mixer source.
    #[must_use]
    pub const fn output_format(&self) -> Option<SourcePcmFormat> {
        self.output_format
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

    /// Returns a redacted low-rate output lifecycle snapshot.
    ///
    /// The projection mirrors [`crate::OutputController::snapshot`] so either
    /// output owner can feed the desktop control snapshot without exposing
    /// device handles or audio material.
    #[must_use]
    pub fn snapshot(&self) -> OutputSnapshot {
        OutputSnapshot::from_health(
            self.selection.is_some(),
            self.stream_active,
            self.fault_reported,
            self.output.health(),
        )
    }

    fn install(&mut self, selection: &OutputSelection) -> Result<(), MixerOutputLifecycleError> {
        selection.config.validate().map_err(|_| {
            MixerOutputLifecycleError::Source(MixerRenderSourceError::InvalidConfig)
        })?;
        let output_format =
            SourcePcmFormat::new(selection.config.sample_rate_hz, selection.config.channels)
                .map_err(|_| {
                    MixerOutputLifecycleError::Source(MixerRenderSourceError::InvalidConfig)
                })?;
        let source = MixerRenderSource::<SLOTS, PCM_SAMPLES_PER_BLOCK>::new(&selection.config)
            .map_err(MixerOutputLifecycleError::Source)?;
        let membership = source.membership();
        self.output
            .start(
                selection.device.as_ref(),
                selection.config,
                Box::new(source),
            )
            .map_err(MixerOutputLifecycleError::Output)?;
        self.membership = Some(membership);
        self.output_format = Some(output_format);
        Ok(())
    }
}

/// Output lifecycle or mixer-source failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MixerOutputLifecycleError {
    /// No successful start has supplied a recovery selection.
    MissingSelection,
    /// A start or recovery was requested while this controller already owns a stream.
    StreamAlreadyActive,
    /// The mixer-backed render source could not be constructed.
    Source(MixerRenderSourceError),
    /// Platform stream operation failed.
    Output(OutputError),
}

impl fmt::Display for MixerOutputLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSelection => {
                formatter.write_str("mixer output recovery has no saved selection")
            }
            Self::StreamAlreadyActive => {
                formatter.write_str("mixer output stream is already active")
            }
            Self::Source(error) => {
                write!(formatter, "mixer render source creation failed: {error}")
            }
            Self::Output(error) => write!(formatter, "mixer output backend failed: {error}"),
        }
    }
}

impl std::error::Error for MixerOutputLifecycleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::MissingSelection | Self::StreamAlreadyActive => None,
            Self::Source(error) => Some(error),
            Self::Output(error) => Some(error),
        }
    }
}

impl<O, const SLOTS: usize, const PCM_SAMPLES_PER_BLOCK: usize> OutputSnapshotSource
    for MixerOutputController<O, SLOTS, PCM_SAMPLES_PER_BLOCK>
where
    O: AudioOutput,
{
    fn snapshot(&self) -> OutputSnapshot {
        MixerOutputController::snapshot(self)
    }
}
