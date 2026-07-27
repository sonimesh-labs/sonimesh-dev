use std::sync::atomic::{AtomicU64, Ordering};

use sonimesh_protocol::{
    Capability, ControlFrameError, Envelope, MAX_CONTROL_CAPABILITIES, StartStream, StopStream,
    envelope, negotiate_hello, validate_control_envelope,
};

use crate::{
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingSource, SessionEvent, SessionState,
};

use super::error::ControlStreamError;
use super::types::{
    ControlOperationToken, ControlSessionToken, ControlStreamAction, MediaStreamIdentity,
    PendingStreamChange, StreamStartRequest,
};

static NEXT_CONTROL_SESSION_TOKEN: AtomicU64 = AtomicU64::new(0);
static NEXT_CONTROL_OPERATION_TOKEN: AtomicU64 = AtomicU64::new(0);

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
    negotiation: Option<sonimesh_protocol::NegotiatedControl>,
    active: Option<MediaStreamIdentity>,
    pending: Option<PendingStreamChange>,
    control_session: Option<ControlSessionToken>,
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
            control_session: None,
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
        self.control_session = None;
    }

    /// Installs one freshly authenticated control transport for the ready session.
    ///
    /// The returned opaque token belongs to that transport instance and must be
    /// supplied with every envelope passed to [`Self::accept`]. Retiring or
    /// reconnecting the session invalidates the token permanently.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::ControlSessionRequiresReady`] unless the
    /// trusted lifecycle is ready, [`ControlStreamError::ControlSessionAlreadyInstalled`]
    /// when a transport is already active, or
    /// [`ControlStreamError::ControlSessionTokenExhausted`] if no fresh token can
    /// be issued.
    pub fn install_authenticated_control_session(
        &mut self,
    ) -> Result<ControlSessionToken, ControlStreamError> {
        if self.state != SessionState::READY {
            return Err(ControlStreamError::ControlSessionRequiresReady(self.state));
        }
        if self.control_session.is_some() {
            return Err(ControlStreamError::ControlSessionAlreadyInstalled);
        }
        let token = Self::issue_control_session_token()?;
        self.control_session = Some(token);
        Ok(token)
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

    /// Admits a pairing source before invoking its TLS-owner proof verifier.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] without invoking `verify` when the
    /// retained invitation or attempt cannot advance the current pairing state.
    pub fn verify_pairing_proof<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        now_ms: u64,
        source: PairingSource,
        verify: impl FnOnce() -> PairingProofVerification,
    ) -> Result<PairingAttemptOutcome, PairingInvitationError> {
        let result = pairing.verify_proof(&mut self.state, now_ms, source, verify);
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

    /// Revokes a retained invitation or trusted pairing session through this owner.
    ///
    /// The pairing controller marks its in-memory record terminal; this owner
    /// then retires control negotiation, active media, and pending completion.
    /// Durable credential and trust-record deletion remains a future platform
    /// pairing-owner responsibility.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when no pairing record is retained or
    /// its lifecycle cannot be synchronized with the control session.
    pub fn revoke_pairing<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
    ) -> Result<crate::PairingRecordState, PairingInvitationError> {
        let result = pairing.revoke(&mut self.state);
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
            SessionEvent::ConnectionLost => {
                self.state
                    .apply(SessionEvent::ConnectionLost)
                    .map_err(ControlStreamError::Transition)?;
                // The transport token and in-flight work belong to the lost
                // authenticated connection and cannot cross the reconnect boundary.
                self.control_session = None;
                self.pending = None;
                Ok(())
            }
            SessionEvent::OutputRestored => self
                .state
                .apply(SessionEvent::OutputRestored)
                .map_err(ControlStreamError::Transition),
            SessionEvent::OutputLost => {
                self.state
                    .apply(SessionEvent::OutputLost)
                    .map_err(ControlStreamError::Transition)?;
                // Fresh output endpoints invalidate in-flight platform or media
                // work. A late completion cannot commit across this boundary.
                self.pending = None;
                Ok(())
            }
            SessionEvent::Reconnected => {
                self.state
                    .apply(SessionEvent::Reconnected)
                    .map_err(ControlStreamError::Transition)?;
                // Reconnect uses fresh transport/session material, so no prior
                // negotiation, media identity, or platform completion can cross it.
                self.negotiation = None;
                self.active = None;
                self.pending = None;
                self.control_session = None;
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
    pub const fn negotiation(&self) -> Option<sonimesh_protocol::NegotiatedControl> {
        self.negotiation
    }

    /// Validates and prepares an action from an authenticated control envelope.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::StaleControlSession`] before envelope
    /// validation when `session` is not the currently installed authenticated
    /// transport token. Otherwise returns [`ControlStreamError::Protocol`] for
    /// invalid envelopes and a lifecycle error for conflicting, stale, or
    /// prematurely completed work.
    pub fn accept(
        &mut self,
        session: ControlSessionToken,
        envelope: &Envelope,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        if self.control_session != Some(session) {
            return Err(ControlStreamError::StaleControlSession);
        }
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
            let token = Self::issue_operation_token()?;
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

        let token = Self::issue_operation_token()?;
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
            let token = Self::issue_operation_token()?;
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

    fn issue_control_session_token() -> Result<ControlSessionToken, ControlStreamError> {
        issue_process_unique_token(
            &NEXT_CONTROL_SESSION_TOKEN,
            ControlStreamError::ControlSessionTokenExhausted,
        )
        .map(ControlSessionToken)
    }

    fn issue_operation_token() -> Result<ControlOperationToken, ControlStreamError> {
        issue_process_unique_token(
            &NEXT_CONTROL_OPERATION_TOKEN,
            ControlStreamError::OperationTokenExhausted,
        )
        .map(ControlOperationToken)
    }

    fn retire_if_idle(&mut self) {
        if self.state == SessionState::IDLE {
            self.negotiation = None;
            self.active = None;
            self.pending = None;
            self.control_session = None;
        }
    }
}

fn issue_process_unique_token(
    counter: &AtomicU64,
    exhausted: ControlStreamError,
) -> Result<u64, ControlStreamError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .map(|previous| previous + 1)
        .map_err(|_| exhausted)
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
