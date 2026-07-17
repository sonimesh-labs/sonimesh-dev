use core::fmt;

use sonimesh_protocol::{
    Capability, ControlFrameError, Envelope, MAX_CONTROL_CAPABILITIES, MediaCryptoError,
    MediaDirection, MediaKeyContext, NegotiatedControl, StartStream, StopStream, envelope,
    negotiate_hello, validate_control_envelope,
};

use crate::{
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingSource, SessionEvent, SessionState, SessionTransitionError,
};

/// Opaque, non-reusable identifier for a pending control-domain operation.
///
/// The coordinator issues this token with a start, rotation, or stop action.
/// The platform owner must return the same token when acknowledging completion
/// or failure, so a late result cannot affect a newer control session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlOperationToken(u64);

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
    /// The authenticated peer hello selected a fixed control capability set.
    Negotiated(NegotiatedControl),
    /// Prepare platform output and stream-local media state for a new stream.
    Start {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Validated stream parameters for platform and media preparation.
        request: StreamStartRequest,
    },
    /// Replace media state without changing the already-running output stream.
    Rotate {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Currently active media identity to retire.
        previous: MediaStreamIdentity,
        /// New epoch and configuration to install.
        next: StreamStartRequest,
    },
    /// Stop the exact active media identity.
    Stop {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Exact media identity to stop.
        identity: MediaStreamIdentity,
    },
    /// Command has no state change because it repeats known completed work.
    Noop,
    /// Envelope is valid but is not a stream lifecycle command for this owner.
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingStreamChange {
    Start {
        token: ControlOperationToken,
        request: StreamStartRequest,
    },
    Rotate {
        token: ControlOperationToken,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    },
    Stop {
        token: ControlOperationToken,
        identity: MediaStreamIdentity,
    },
}

impl PendingStreamChange {
    const fn identity(self) -> MediaStreamIdentity {
        match self {
            Self::Start { request, .. } => request.identity,
            Self::Rotate { next, .. } => next.identity,
            Self::Stop { identity, .. } => identity,
        }
    }

    const fn token(self) -> ControlOperationToken {
        match self {
            Self::Start { token, .. } | Self::Rotate { token, .. } | Self::Stop { token, .. } => {
                token
            }
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
    /// The local policy exceeds the protocol's fixed capability bound.
    LocalCapabilityPolicyTooLarge,
    /// The authenticated peer hello could not select a compatible control session.
    Negotiation(sonimesh_protocol::ControlNegotiationError),
    /// A hello was received after a capability set was already fixed.
    Renegotiation,
    /// An authenticated hello arrived before pairing established a ready session.
    HelloRequiresReady(SessionState),
    /// A stream command arrived before an authenticated hello selected capabilities.
    MissingNegotiation,
    /// Start requested a frame duration that peers did not negotiate.
    UnsupportedFrameDuration(u32),
    /// A start attempted to move a stream back to an earlier epoch.
    StaleEpoch {
        /// Existing active key epoch.
        active: u16,
        /// Rejected key epoch from the command.
        requested: u16,
    },
    /// Completion did not correspond to the prepared action or current state.
    CompletionMismatch,
    /// A control-domain lifecycle acknowledgement was invalid for this state.
    Transition(SessionTransitionError),
    /// This event is owned by a dedicated coordinator operation.
    UnsupportedLifecycleEvent(SessionEvent),
    /// The coordinator cannot issue another non-reusable operation token.
    OperationTokenExhausted,
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
            Self::LocalCapabilityPolicyTooLarge => {
                formatter.write_str("local control capability policy exceeds the hard limit")
            }
            Self::Negotiation(error) => write!(formatter, "control negotiation failed: {error}"),
            Self::Renegotiation => {
                formatter.write_str("control capabilities were already negotiated")
            }
            Self::HelloRequiresReady(state) => {
                write!(
                    formatter,
                    "control hello requires Ready state, found {state:?}"
                )
            }
            Self::MissingNegotiation => {
                formatter.write_str("stream command requires a negotiated control hello")
            }
            Self::UnsupportedFrameDuration(duration) => {
                write!(formatter, "frame duration {duration} ms was not negotiated")
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
            Self::Transition(error) => {
                write!(formatter, "control lifecycle transition failed: {error}")
            }
            Self::UnsupportedLifecycleEvent(event) => {
                write!(
                    formatter,
                    "session event {event:?} is not a platform acknowledgement"
                )
            }
            Self::OperationTokenExhausted => {
                formatter.write_str("control operation token sequence is exhausted")
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
    local_capabilities: [Capability; MAX_CONTROL_CAPABILITIES],
    local_capability_count: usize,
    negotiation: Option<NegotiatedControl>,
    active: Option<MediaStreamIdentity>,
    pending: Option<PendingStreamChange>,
    next_operation_token: u64,
}

impl ControlStreamCoordinator {
    /// Creates a control coordinator with a bounded local capability policy.
    ///
    /// The caller must pass only policy selected by the local application; the
    /// peer's authenticated hello is accepted later through [`Self::accept`].
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::LocalCapabilityPolicyTooLarge`] before
    /// storing any state when `local_capabilities` exceeds the protocol bound.
    pub fn new(
        state: SessionState,
        local_capabilities: &[Capability],
    ) -> Result<Self, ControlStreamError> {
        if local_capabilities.len() > MAX_CONTROL_CAPABILITIES {
            return Err(ControlStreamError::LocalCapabilityPolicyTooLarge);
        }
        let mut capabilities = [Capability::Unspecified; MAX_CONTROL_CAPABILITIES];
        capabilities[..local_capabilities.len()].copy_from_slice(local_capabilities);
        Ok(Self {
            state,
            local_capabilities: capabilities,
            local_capability_count: local_capabilities.len(),
            negotiation: None,
            active: None,
            pending: None,
            next_operation_token: 0,
        })
    }

    /// Returns the current shared control-domain lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// Retires the authenticated control session and all of its stream state.
    ///
    /// Call this when trust, reconnect grace, or the transport session expires.
    /// It intentionally invalidates any pending completion so a late platform or
    /// media operation cannot revive a retired capability set or stream identity.
    pub fn expire_control_session(&mut self) {
        self.state = SessionState::IDLE;
        self.negotiation = None;
        self.active = None;
        self.pending = None;
    }

    /// Starts a pairing invitation while keeping lifecycle ownership centralized.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when the invitation record or current
    /// session state rejects the requested pairing operation.
    pub fn begin_pairing<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        id: PairingId,
        now_ms: u64,
        lifetime_ms: u64,
        max_attempts_per_source: u8,
    ) -> Result<(), PairingInvitationError> {
        pairing.begin(
            &mut self.state,
            id,
            now_ms,
            lifetime_ms,
            max_attempts_per_source,
        )
    }

    /// Records a TLS-owner verified pairing proof through this session owner.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when the retained invitation or proof
    /// attempt cannot advance the current pairing state.
    pub fn record_pairing_proof<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        now_ms: u64,
        source: PairingSource,
        proof_verified: bool,
    ) -> Result<PairingAttemptOutcome, PairingInvitationError> {
        let result = pairing.record_proof(&mut self.state, now_ms, source, proof_verified);
        self.retire_if_idle();
        result
    }

    /// Polls pairing expiry and retires every session-owned value if it reaches idle.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when no invitation is retained or its
    /// monotonic-clock observation is invalid. Session-owned state is retired
    /// before returning whenever observation moves the lifecycle to idle.
    pub fn observe_pairing<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        now_ms: u64,
    ) -> Result<crate::PairingRecordState, PairingInvitationError> {
        let result = pairing.observe(&mut self.state, now_ms);
        self.retire_if_idle();
        result
    }

    /// Expires an active pairing invitation through this lifecycle owner.
    ///
    /// Trusted control-session expiry is handled by
    /// [`Self::expire_control_session`]; a consumed invitation is retained for
    /// pairing audit state and cannot be expired as though it were active.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when there is no retained invitation
    /// or it is no longer an active invitation.
    pub fn expire_pairing<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
    ) -> Result<crate::PairingRecordState, PairingInvitationError> {
        if pairing.state() != Some(crate::PairingRecordState::Active) {
            return Err(PairingInvitationError::InvalidSessionState(self.state));
        }
        let result = pairing.expire(&mut self.state);
        self.retire_if_idle();
        result
    }

    /// Applies a platform acknowledgement that does not create, stop, or expire a session.
    ///
    /// Stream start and stop are committed by their tokenized completion methods;
    /// pairing and session expiry have dedicated owner methods.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::Transition`] when the current state rejects
    /// the platform acknowledgement, or
    /// [`ControlStreamError::UnsupportedLifecycleEvent`] when callers try to
    /// bypass a dedicated lifecycle owner.
    pub fn acknowledge_platform_event(
        &mut self,
        event: SessionEvent,
    ) -> Result<(), ControlStreamError> {
        match event {
            SessionEvent::ConnectionLost
            | SessionEvent::OutputLost
            | SessionEvent::OutputRestored => self
                .state
                .apply(event)
                .map_err(ControlStreamError::Transition),
            SessionEvent::Reconnected => {
                self.state
                    .apply(SessionEvent::Reconnected)
                    .map_err(ControlStreamError::Transition)?;
                // Reconnect uses fresh transport/session material, so no prior
                // negotiation, media identity, or platform completion can cross it.
                self.negotiation = None;
                self.active = None;
                self.pending = None;
                Ok(())
            }
            SessionEvent::StartPairing
            | SessionEvent::StartStream
            | SessionEvent::StopStream
            | SessionEvent::SessionExpired => {
                Err(ControlStreamError::UnsupportedLifecycleEvent(event))
            }
        }
    }

    /// Returns the exact currently active media identity, if any.
    #[must_use]
    pub const fn active(&self) -> Option<MediaStreamIdentity> {
        self.active
    }

    /// Returns the one-time negotiated control capability set, if available.
    #[must_use]
    pub const fn negotiation(&self) -> Option<NegotiatedControl> {
        self.negotiation
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
            Some(envelope::Payload::Hello(hello)) => self.accept_hello(hello),
            Some(envelope::Payload::StartStream(start)) => self.accept_start(start),
            Some(envelope::Payload::StopStream(stop)) => self.accept_stop(stop),
            Some(envelope::Payload::StreamStatus(_) | envelope::Payload::Error(_)) => {
                Ok(ControlStreamAction::Ignored)
            }
            None => Err(ControlStreamError::Protocol(
                ControlFrameError::MissingPayload,
            )),
        }
    }

    /// Records successful output/media installation after a tokenized start action.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] unless this exact
    /// start remains pending. The coordinator advances the lifecycle to
    /// `Streaming` only after this platform acknowledgement.
    pub fn complete_start(
        &mut self,
        token: ControlOperationToken,
        request: StreamStartRequest,
    ) -> Result<(), ControlStreamError> {
        if self.pending != Some(PendingStreamChange::Start { token, request }) {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.state
            .apply(SessionEvent::StartStream)
            .map_err(ControlStreamError::Transition)?;
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
        token: ControlOperationToken,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    ) -> Result<(), ControlStreamError> {
        if self.pending
            != Some(PendingStreamChange::Rotate {
                token,
                previous,
                next,
            })
            || self.active != Some(previous)
            || self.state != SessionState::STREAMING
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.active = Some(next.identity);
        self.pending = None;
        Ok(())
    }

    /// Records output shutdown after a tokenized stop action.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] unless the matching
    /// stop remains pending. The coordinator enters `Ready` only after this
    /// platform acknowledgement.
    pub fn complete_stop(
        &mut self,
        token: ControlOperationToken,
        identity: MediaStreamIdentity,
    ) -> Result<(), ControlStreamError> {
        if self.pending != Some(PendingStreamChange::Stop { token, identity })
            || self.active != Some(identity)
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.state
            .apply(SessionEvent::StopStream)
            .map_err(ControlStreamError::Transition)?;
        self.active = None;
        self.pending = None;
        Ok(())
    }

    /// Abandons a failed platform start or media rotation without changing active media.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::CompletionMismatch`] when no matching
    /// action is pending.
    pub fn abort_pending(
        &mut self,
        token: ControlOperationToken,
    ) -> Result<(), ControlStreamError> {
        if self.pending.is_none_or(|pending| pending.token() != token) {
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
        if !self.supports_frame_duration(request.frame_duration_ms)? {
            return Err(ControlStreamError::UnsupportedFrameDuration(
                request.frame_duration_ms,
            ));
        }
        if let Some(pending) = self.pending {
            return if pending.identity() == request.identity {
                Ok(ControlStreamAction::Noop)
            } else {
                Err(ControlStreamError::PendingChange)
            };
        }

        let Some(active) = self.active else {
            if self.state != SessionState::READY {
                return Err(ControlStreamError::StartRequiresReady(self.state));
            }
            let token = self.issue_operation_token()?;
            self.pending = Some(PendingStreamChange::Start { token, request });
            return Ok(ControlStreamAction::Start { token, request });
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
        if self.state != SessionState::STREAMING {
            return Err(ControlStreamError::RotationRequiresStreaming(self.state));
        }

        let token = self.issue_operation_token()?;
        self.pending = Some(PendingStreamChange::Rotate {
            token,
            previous: active,
            next: request,
        });
        Ok(ControlStreamAction::Rotate {
            token,
            previous: active,
            next: request,
        })
    }

    fn accept_stop(
        &mut self,
        stop: &StopStream,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        let identity = identity_from_stop(stop)?;
        if let Some(pending) = self.pending {
            return if matches!(pending, PendingStreamChange::Stop { identity: pending_identity, .. } if pending_identity == identity)
            {
                Ok(ControlStreamAction::Noop)
            } else {
                Err(ControlStreamError::PendingChange)
            };
        }
        if self.active == Some(identity) {
            let token = self.issue_operation_token()?;
            self.pending = Some(PendingStreamChange::Stop { token, identity });
            Ok(ControlStreamAction::Stop { token, identity })
        } else {
            Ok(ControlStreamAction::Noop)
        }
    }

    fn accept_hello(
        &mut self,
        hello: &sonimesh_protocol::Hello,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        if self.negotiation.is_some() {
            return Err(ControlStreamError::Renegotiation);
        }
        if self.state != SessionState::READY {
            return Err(ControlStreamError::HelloRequiresReady(self.state));
        }
        let negotiation = negotiate_hello(
            hello,
            &self.local_capabilities[..self.local_capability_count],
        )
        .map_err(ControlStreamError::Negotiation)?;
        self.negotiation = Some(negotiation);
        Ok(ControlStreamAction::Negotiated(negotiation))
    }

    fn supports_frame_duration(&self, duration_ms: u32) -> Result<bool, ControlStreamError> {
        let negotiation = self
            .negotiation
            .ok_or(ControlStreamError::MissingNegotiation)?;
        let capability = match duration_ms {
            10 => Capability::FrameDuration10Ms,
            20 => Capability::FrameDuration20Ms,
            _ => return Ok(false),
        };
        Ok(negotiation.supports(capability))
    }

    fn issue_operation_token(&mut self) -> Result<ControlOperationToken, ControlStreamError> {
        self.next_operation_token = self
            .next_operation_token
            .checked_add(1)
            .ok_or(ControlStreamError::OperationTokenExhausted)?;
        Ok(ControlOperationToken(self.next_operation_token))
    }

    fn retire_if_idle(&mut self) {
        if self.state == SessionState::IDLE {
            self.negotiation = None;
            self.active = None;
            self.pending = None;
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
