use core::fmt;

use sonimesh_protocol::{
    ControlFrameError, Envelope, MediaCryptoError, MediaDirection, MediaKeyContext, StartStream,
    StopStream, envelope, validate_control_envelope,
};

use crate::SessionState;

/// Exact media identity bound to one session, stream, and AEAD key epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaStreamIdentity {
    /// Random identifier for the active media session.
    pub session_id: u64,
    /// Stream identifier within the media session.
    pub stream_id: u32,
    /// Explicit AEAD key generation for this stream.
    pub key_epoch: u16,
}

impl MediaStreamIdentity {
    /// Builds the protocol key context for one fixed media direction.
    ///
    /// This copies only public media identity fields. The authenticated root
    /// secret remains owned by the pairing/control-session layer.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::InvalidContext`] if a caller manually
    /// constructs an identity with a zero session or stream ID.
    pub const fn media_key_context(
        self,
        direction: MediaDirection,
    ) -> Result<MediaKeyContext, MediaCryptoError> {
        MediaKeyContext::new(self.session_id, self.stream_id, self.key_epoch, direction)
    }
}

/// Validated audio parameters carried by a `StartStream` control command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamStartRequest {
    /// Media identity to install after platform/output preparation succeeds.
    pub identity: MediaStreamIdentity,
    /// Sender PCM sample rate to configure for the stream pipeline.
    pub sample_rate_hz: u32,
    /// Sender channel count to configure for the stream pipeline.
    pub channels: u32,
    /// Encoded frame duration selected by control negotiation.
    pub frame_duration_ms: u32,
}

/// Low-rate action prepared from one authenticated control envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlStreamAction {
    /// Prepare platform output and stream-local media state for a new stream.
    Start(StreamStartRequest),
    /// Replace media state without changing the already-running output stream.
    Rotate {
        /// Currently active media identity to retire.
        previous: MediaStreamIdentity,
        /// New epoch and configuration to install.
        next: StreamStartRequest,
    },
    /// Stop the exact active media identity.
    Stop(MediaStreamIdentity),
    /// Command has no state change because it repeats known completed work.
    Noop,
    /// Envelope is valid but is not a stream lifecycle command for this owner.
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingStreamChange {
    Start(StreamStartRequest),
    Rotate {
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    },
}

impl PendingStreamChange {
    const fn identity(self) -> MediaStreamIdentity {
        match self {
            Self::Start(request) => request.identity,
            Self::Rotate { next, .. } => next.identity,
        }
    }
}

/// Failure while coordinating a validated control command with lifecycle state.
#[derive(Debug)]
pub enum ControlStreamError {
    /// Envelope exceeded the protocol framing or semantic contract.
    Protocol(ControlFrameError),
    /// A protocol-valid epoch could not convert to the media `u16` type.
    InvalidEpoch,
    /// A new stream was requested while another session/stream is active.
    ActiveStreamConflict,
    /// A pending platform or media change must finish before another one begins.
    PendingChange,
    /// A start command arrived in a state that cannot prepare output.
    StartRequiresReady(SessionState),
    /// A key rotation arrived outside active streaming.
    RotationRequiresStreaming(SessionState),
    /// A start attempted to move a stream back to an earlier epoch.
    StaleEpoch {
        /// Existing active key epoch.
        active: u16,
        /// Rejected key epoch from the command.
        requested: u16,
    },
    /// Completion did not correspond to the prepared action or current state.
    CompletionMismatch,
}

impl fmt::Display for ControlStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(error) => write!(formatter, "control command rejected: {error}"),
            Self::InvalidEpoch => formatter.write_str("control epoch cannot fit media key context"),
            Self::ActiveStreamConflict => {
                formatter.write_str("a different media stream is already active")
            }
            Self::PendingChange => formatter.write_str("a stream lifecycle change is pending"),
            Self::StartRequiresReady(state) => {
                write!(
                    formatter,
                    "stream start requires Ready state, found {state:?}"
                )
            }
            Self::RotationRequiresStreaming(state) => {
                write!(
                    formatter,
                    "key rotation requires Streaming state, found {state:?}"
                )
            }
            Self::StaleEpoch { active, requested } => {
                write!(
                    formatter,
                    "media key epoch {requested} is older than active epoch {active}"
                )
            }
            Self::CompletionMismatch => {
                formatter.write_str("stream lifecycle completion did not match pending state")
            }
        }
    }
}

impl std::error::Error for ControlStreamError {}

/// Control-domain coordinator for one desktop receiver stream.
///
/// The coordinator only accepts frames after the transport authenticates its
/// control session. It never receives media packets, exposes key material, or
/// invokes a platform backend. Callers execute returned actions, advance the
/// shared [`SessionState`] through their platform owner, then acknowledge the
/// completed action here.
pub struct ControlStreamCoordinator {
    state: SessionState,
    active: Option<MediaStreamIdentity>,
    pending: Option<PendingStreamChange>,
}

impl ControlStreamCoordinator {
    /// Creates a control coordinator in the supplied lifecycle state.
    #[must_use]
    pub const fn new(state: SessionState) -> Self {
        Self {
            state,
            active: None,
            pending: None,
        }
    }

    /// Returns the current shared control-domain lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Borrows lifecycle state for a platform operation outside the output callback.
    pub fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    /// Returns the exact currently active media identity, if any.
    #[must_use]
    pub const fn active(&self) -> Option<MediaStreamIdentity> {
        self.active
    }

    /// Validates and prepares an action from an authenticated control envelope.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::Protocol`] for invalid envelopes and a
    /// lifecycle error for conflicting, stale, or prematurely completed work.
    pub fn accept(
        &mut self,
        envelope: &Envelope,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        validate_control_envelope(envelope).map_err(ControlStreamError::Protocol)?;
        match envelope.payload.as_ref() {
            Some(envelope::Payload::StartStream(start)) => self.accept_start(start),
            Some(envelope::Payload::StopStream(stop)) => self.accept_stop(stop),
            Some(
                envelope::Payload::Hello(_)
                | envelope::Payload::StreamStatus(_)
                | envelope::Payload::Error(_),
            ) => Ok(ControlStreamAction::Ignored),
            None => Err(ControlStreamError::Protocol(
                ControlFrameError::MissingPayload,
            )),
        }
    }

    /// Records successful output/media installation after a [`ControlStreamAction::Start`].
    ///
    /// The platform owner must have changed [`Self::state`] to `Streaming`
    /// before this call.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] unless this exact
    /// start was pending and the platform owner reached `Streaming`.
    pub fn complete_start(
        &mut self,
        request: StreamStartRequest,
    ) -> Result<(), ControlStreamError> {
        if self.pending != Some(PendingStreamChange::Start(request))
            || self.state != SessionState::Streaming
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.active = Some(request.identity);
        self.pending = None;
        Ok(())
    }

    /// Records successful stream-local replacement after a rotation action.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] unless the pending
    /// rotation, active identity, and lifecycle state all still match.
    pub fn complete_rotation(
        &mut self,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    ) -> Result<(), ControlStreamError> {
        if self.pending != Some(PendingStreamChange::Rotate { previous, next })
            || self.active != Some(previous)
            || self.state != SessionState::Streaming
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.active = Some(next.identity);
        self.pending = None;
        Ok(())
    }

    /// Records output shutdown after a [`ControlStreamAction::Stop`].
    ///
    /// The platform owner must have changed [`Self::state`] to `Ready` before
    /// this call, preserving the output controller's existing atomicity rule.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] unless the matching
    /// active stream stopped and the platform owner reached `Ready`.
    pub fn complete_stop(
        &mut self,
        identity: MediaStreamIdentity,
    ) -> Result<(), ControlStreamError> {
        if self.active != Some(identity)
            || self.state != SessionState::Ready
            || self.pending.is_some()
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.active = None;
        Ok(())
    }

    /// Abandons a failed platform start or media rotation without changing active media.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] when no matching
    /// start or rotation action is pending.
    pub fn abort_pending(
        &mut self,
        identity: MediaStreamIdentity,
    ) -> Result<(), ControlStreamError> {
        if self
            .pending
            .is_none_or(|pending| pending.identity() != identity)
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.pending = None;
        Ok(())
    }

    fn accept_start(
        &mut self,
        start: &StartStream,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        let request = request_from_start(start)?;
        if let Some(pending) = self.pending {
            return if pending.identity() == request.identity {
                Ok(ControlStreamAction::Noop)
            } else {
                Err(ControlStreamError::PendingChange)
            };
        }

        let Some(active) = self.active else {
            if self.state != SessionState::Ready {
                return Err(ControlStreamError::StartRequiresReady(self.state));
            }
            self.pending = Some(PendingStreamChange::Start(request));
            return Ok(ControlStreamAction::Start(request));
        };

        if active.session_id != request.identity.session_id
            || active.stream_id != request.identity.stream_id
        {
            return Err(ControlStreamError::ActiveStreamConflict);
        }
        if request.identity.key_epoch == active.key_epoch {
            return Ok(ControlStreamAction::Noop);
        }
        if request.identity.key_epoch < active.key_epoch {
            return Err(ControlStreamError::StaleEpoch {
                active: active.key_epoch,
                requested: request.identity.key_epoch,
            });
        }
        if self.state != SessionState::Streaming {
            return Err(ControlStreamError::RotationRequiresStreaming(self.state));
        }

        self.pending = Some(PendingStreamChange::Rotate {
            previous: active,
            next: request,
        });
        Ok(ControlStreamAction::Rotate {
            previous: active,
            next: request,
        })
    }

    fn accept_stop(&self, stop: &StopStream) -> Result<ControlStreamAction, ControlStreamError> {
        let identity = identity_from_stop(stop)?;
        if self.pending.is_some() {
            return Err(ControlStreamError::PendingChange);
        }
        if self.active == Some(identity) {
            Ok(ControlStreamAction::Stop(identity))
        } else {
            Ok(ControlStreamAction::Noop)
        }
    }
}

fn request_from_start(start: &StartStream) -> Result<StreamStartRequest, ControlStreamError> {
    Ok(StreamStartRequest {
        identity: MediaStreamIdentity {
            session_id: start.media_session_id,
            stream_id: start.stream_id,
            key_epoch: u16::try_from(start.media_key_epoch)
                .map_err(|_| ControlStreamError::InvalidEpoch)?,
        },
        sample_rate_hz: start.sample_rate_hz,
        channels: start.channels,
        frame_duration_ms: start.frame_duration_ms,
    })
}

fn identity_from_stop(stop: &StopStream) -> Result<MediaStreamIdentity, ControlStreamError> {
    Ok(MediaStreamIdentity {
        session_id: stop.media_session_id,
        stream_id: stop.stream_id,
        key_epoch: u16::try_from(stop.media_key_epoch)
            .map_err(|_| ControlStreamError::InvalidEpoch)?,
    })
}
