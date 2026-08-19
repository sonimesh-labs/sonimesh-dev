use std::sync::atomic::{AtomicU64, Ordering};

use sonimesh_protocol::{
    Capability, ControlFrameError, Envelope, MAX_CONTROL_CAPABILITIES, StartStream, StopStream,
    envelope, negotiate_hello, validate_control_envelope,
};

use crate::{
    DesktopControlSnapshot, OutputSnapshotSource, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingInvitationError, PairingProofVerification, PairingSource,
    ReconnectGraceController, ReconnectGraceError, ReconnectGraceObservation, ReconnectGraceState,
    SessionEvent, SessionState,
};

use super::error::ControlStreamError;
use super::types::{
    ControlOperationToken, ControlSessionToken, ControlStreamAction, MediaStreamIdentity,
    PendingStreamChange, StreamStartRequest,
};

static NEXT_CONTROL_SESSION_TOKEN: AtomicU64 = AtomicU64::new(0);
static NEXT_CONTROL_OPERATION_TOKEN: AtomicU64 = AtomicU64::new(0);

/// Maximum concurrently active media streams in one control session.
///
/// Provisionally matches the Phase 2 mixer slot bound (ADR 0015): headroom
/// above the roadmap's four-source maximum, with admission failing closed at
/// the bound in the control domain before any platform work is prepared.
pub const MAX_ACTIVE_STREAMS: usize = 8;

/// Control-domain coordinator for the desktop receiver's control plane.
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
    active: [Option<StreamStartRequest>; MAX_ACTIVE_STREAMS],
    pending: Option<PendingStreamChange>,
    control_session: Option<ControlSessionToken>,
    reconnect_grace: ReconnectGraceController,
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
            active: [const { None }; MAX_ACTIVE_STREAMS],
            pending: None,
            control_session: None,
            reconnect_grace: ReconnectGraceController::new(),
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
        self.active = [const { None }; MAX_ACTIVE_STREAMS];
        self.pending = None;
        self.control_session = None;
        let _ = self.reconnect_grace.cancel();
    }

    /// Arms the bounded reconnect window after a connection loss.
    ///
    /// The window is clock-driven: the control-session owner polls
    /// [`Self::observe_reconnect_grace`] with its monotonic clock and must
    /// expire the session when the deadline elapses.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::GraceRequiresReconnecting`] unless the
    /// lifecycle is in [`SessionState::RECONNECTING`], or a
    /// [`ControlStreamError::ReconnectGrace`] error when the window cannot be
    /// armed.
    pub fn begin_reconnect_grace(
        &mut self,
        now_ms: u64,
        duration_ms: u64,
    ) -> Result<(), ControlStreamError> {
        if self.state != SessionState::RECONNECTING {
            return Err(ControlStreamError::GraceRequiresReconnecting(self.state));
        }
        self.reconnect_grace
            .begin(now_ms, duration_ms)
            .map_err(ControlStreamError::ReconnectGrace)
    }

    /// Polls the reconnect window and expires the session when it elapses.
    ///
    /// On expiry the coordinator retires every session-owned value and returns
    /// [`ReconnectGraceObservation::Expired`], so the caller cannot observe a
    /// lingering ready or streaming session after the deadline. A clock
    /// regression fails closed by expiring the session before returning the
    /// error.
    ///
    /// # Errors
    ///
    /// Returns [`ControlStreamError::ReconnectGrace`] with
    /// [`ReconnectGraceError::TimeRegressed`] when the clock regresses.
    pub fn observe_reconnect_grace(
        &mut self,
        now_ms: u64,
    ) -> Result<ReconnectGraceObservation, ControlStreamError> {
        let observation = self
            .reconnect_grace
            .observe(now_ms)
            .map_err(ControlStreamError::ReconnectGrace);
        let expired = matches!(
            observation,
            Ok(ReconnectGraceObservation::Expired)
                | Err(ControlStreamError::ReconnectGrace(
                    ReconnectGraceError::TimeRegressed
                ))
        );
        if expired {
            self.expire_control_session();
        }
        observation
    }

    /// Returns the current reconnect-window lifecycle state.
    #[must_use]
    pub const fn reconnect_grace_state(&self) -> ReconnectGraceState {
        self.reconnect_grace.state()
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

    /// Projects the current redacted desktop control snapshot through the pairing owner.
    ///
    /// The pairing owner applies its monotonic clock before projecting, so a
    /// bridge cannot present an active invitation without first observing
    /// expiry. This entry point also observes the bounded reconnect-grace
    /// clock first, so a bridge that only polls snapshots sees a reconnecting
    /// session retire once its deadline passes and never observes a stale
    /// countdown. The output snapshot is read directly from the supplied
    /// output owner, and session-owned state is retired before returning
    /// whenever the observation moves the lifecycle to idle.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] when the monotonic-clock observation
    /// is invalid. An owner with no retained invitation still returns a snapshot.
    pub fn snapshot_at<const MAX_SOURCES: usize>(
        &mut self,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
        now_ms: u64,
        output: &impl OutputSnapshotSource,
    ) -> Result<DesktopControlSnapshot, PairingInvitationError> {
        let reconnect_remaining_ms = self.snapshot_reconnect_remaining_ms(now_ms);
        let result = pairing.snapshot_at(&mut self.state, now_ms, output);
        self.retire_if_idle();
        result.map(|snapshot| snapshot.with_reconnect_remaining_ms(reconnect_remaining_ms))
    }

    /// Observes the reconnect window with the snapshot clock and returns the
    /// bounded remaining milliseconds.
    ///
    /// The snapshot entry point advances both the pairing invitation clock and
    /// the reconnect-window clock, so a bridge that only polls snapshots still
    /// sees a reconnecting session retire once its deadline passes. A clock
    /// regression or deadline already force-expires the window and retires the
    /// control session, so the projection fails closed with no remaining time.
    fn snapshot_reconnect_remaining_ms(&mut self, now_ms: u64) -> Option<u64> {
        match self.observe_reconnect_grace(now_ms) {
            Ok(ReconnectGraceObservation::InGrace { remaining_ms }) => Some(remaining_ms),
            Ok(ReconnectGraceObservation::Idle | ReconnectGraceObservation::Expired) | Err(_) => {
                None
            }
        }
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
                self.active = [const { None }; MAX_ACTIVE_STREAMS];
                self.pending = None;
                self.control_session = None;
                // Fresh session material ends any armed reconnect window.
                let _ = self.reconnect_grace.reconnected();
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

    /// Returns the primary active media identity, if any.
    ///
    /// The primary stream is the first-admitted active stream. Single-stream
    /// callers keep the historical contract: this is the only active stream.
    /// Multi-stream callers should use [`Self::active_streams`] and
    /// [`Self::stream_count`].
    #[must_use]
    pub const fn active(&self) -> Option<MediaStreamIdentity> {
        let mut index = 0;
        while index < MAX_ACTIVE_STREAMS {
            if let Some(request) = self.active[index] {
                return Some(request.identity);
            }
            index += 1;
        }
        None
    }

    /// Number of concurrently active media streams.
    #[must_use]
    pub fn stream_count(&self) -> usize {
        self.active.iter().flatten().count()
    }

    /// Iterates every active media identity in admission order.
    pub fn active_streams(&self) -> impl Iterator<Item = MediaStreamIdentity> + '_ {
        self.active.iter().flatten().map(|request| request.identity)
    }

    /// Returns the one-time negotiated control capability set, if available.
    #[must_use]
    pub const fn negotiation(&self) -> Option<sonimesh_protocol::NegotiatedControl> {
        self.negotiation
    }

    /// Returns whether `session` is the currently installed authenticated
    /// control transport.
    ///
    /// This is intentionally a non-mutating gate for application-domain
    /// composition code that must authorize work without fabricating a
    /// control envelope. A retired or superseded token is never accepted.
    #[must_use]
    pub fn has_authenticated_control_session(&self, session: ControlSessionToken) -> bool {
        self.control_session == Some(session)
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
        if self.active_count() >= MAX_ACTIVE_STREAMS {
            return Err(ControlStreamError::ActiveStreamLimit);
        }
        // The first admitted stream moves the lifecycle to Streaming; additional
        // streams keep the already-streaming session.
        if self.active_count() == 0 {
            self.state
                .apply(SessionEvent::StartStream)
                .map_err(ControlStreamError::Transition)?;
        }
        self.insert_active(request)?;
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
            || !self.is_active(previous)
            || self.state != SessionState::STREAMING
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.replace_active(previous, next);
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
            || !self.is_active(identity)
        {
            return Err(ControlStreamError::CompletionMismatch);
        }
        self.remove_active(identity);
        // The session returns to Ready only after the last active stream stops.
        if self.active_count() == 0 {
            self.state
                .apply(SessionEvent::StopStream)
                .map_err(ControlStreamError::Transition)?;
        }
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
            let Some(pending_request) = pending.start_request() else {
                return Err(ControlStreamError::PendingChange);
            };
            return if pending_request == request {
                Ok(ControlStreamAction::Noop)
            } else if pending_request.identity == request.identity {
                Err(ControlStreamError::StreamConfigurationConflict)
            } else {
                Err(ControlStreamError::PendingChange)
            };
        }

        if let Some(active_request) = self.active_request(request.identity) {
            // The identity is already active: an identical command is a no-op,
            // a conflicting configuration is rejected, and a strictly newer key
            // epoch rotates only this stream.
            let active = active_request.identity;
            if request.identity.key_epoch == active.key_epoch {
                return if request == active_request {
                    Ok(ControlStreamAction::Noop)
                } else {
                    Err(ControlStreamError::StreamConfigurationConflict)
                };
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
            return Ok(ControlStreamAction::Rotate {
                token,
                previous: active,
                next: request,
            });
        }

        // New identity: the first stream starts from Ready, additional streams
        // start while already Streaming. Both fail closed at the slot bound.
        if self.active_count() == 0 {
            if self.state != SessionState::READY {
                return Err(ControlStreamError::StartRequiresReady(self.state));
            }
        } else if self.state != SessionState::STREAMING {
            return Err(ControlStreamError::StartRequiresStreaming(self.state));
        }
        if self.active_count() >= MAX_ACTIVE_STREAMS {
            return Err(ControlStreamError::ActiveStreamLimit);
        }
        let token = Self::issue_operation_token()?;
        self.pending = Some(PendingStreamChange::Start { token, request });
        Ok(ControlStreamAction::Start { token, request })
    }

    fn accept_stop(
        &mut self,
        stop: &StopStream,
    ) -> Result<ControlStreamAction, ControlStreamError> {
        self.require_negotiation()?;
        let identity = identity_from_stop(stop)?;
        if let Some(pending) = self.pending {
            return if matches!(pending, PendingStreamChange::Stop { identity: pending_identity, .. } if pending_identity == identity)
            {
                Ok(ControlStreamAction::Noop)
            } else {
                Err(ControlStreamError::PendingChange)
            };
        }
        if self.is_active(identity) {
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
        let negotiation = self.require_negotiation()?;
        let capability = match duration_ms {
            10 => Capability::FrameDuration10Ms,
            20 => Capability::FrameDuration20Ms,
            _ => return Ok(false),
        };
        Ok(negotiation.supports(capability))
    }

    fn require_negotiation(
        &self,
    ) -> Result<sonimesh_protocol::NegotiatedControl, ControlStreamError> {
        self.negotiation
            .ok_or(ControlStreamError::MissingNegotiation)
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
            self.active = [const { None }; MAX_ACTIVE_STREAMS];
            self.pending = None;
            self.control_session = None;
            let _ = self.reconnect_grace.cancel();
        }
    }

    fn active_count(&self) -> usize {
        self.active.iter().flatten().count()
    }

    fn stream_key_slot(&self, identity: MediaStreamIdentity) -> Option<usize> {
        self.active.iter().position(|slot| {
            slot.is_some_and(|request| same_stream_key(request.identity, identity))
        })
    }

    fn exact_slot_of(&self, identity: MediaStreamIdentity) -> Option<usize> {
        self.active
            .iter()
            .position(|slot| slot.is_some_and(|request| request.identity == identity))
    }

    fn is_active(&self, identity: MediaStreamIdentity) -> bool {
        self.exact_slot_of(identity).is_some()
    }

    fn active_request(&self, identity: MediaStreamIdentity) -> Option<StreamStartRequest> {
        self.active
            .get(self.stream_key_slot(identity)?)
            .copied()
            .flatten()
    }

    fn insert_active(&mut self, request: StreamStartRequest) -> Result<(), ControlStreamError> {
        let Some(slot) = self.active.iter_mut().find(|slot| slot.is_none()) else {
            return Err(ControlStreamError::ActiveStreamLimit);
        };
        *slot = Some(request);
        Ok(())
    }

    fn remove_active(&mut self, identity: MediaStreamIdentity) {
        if let Some(slot) = self.exact_slot_of(identity) {
            self.active[slot] = None;
        }
    }

    fn replace_active(&mut self, identity: MediaStreamIdentity, next: StreamStartRequest) {
        if let Some(slot) = self.exact_slot_of(identity) {
            self.active[slot] = Some(next);
        }
    }
}

fn same_stream_key(left: MediaStreamIdentity, right: MediaStreamIdentity) -> bool {
    left.session_id == right.session_id && left.stream_id == right.stream_id
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

#[cfg(test)]
mod token_tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{ControlStreamError, issue_process_unique_token};

    #[test]
    fn process_unique_tokens_fail_closed_after_issuing_u64_max_once() {
        let counter = AtomicU64::new(u64::MAX - 1);

        assert!(matches!(
            issue_process_unique_token(&counter, ControlStreamError::OperationTokenExhausted),
            Ok(value) if value == u64::MAX
        ));
        assert!(matches!(
            issue_process_unique_token(&counter, ControlStreamError::OperationTokenExhausted),
            Err(ControlStreamError::OperationTokenExhausted)
        ));
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::ControlStreamCoordinator;
    use crate::{
        OutputSnapshot, OutputSnapshotSource, PairingAttemptOutcome, PairingId,
        PairingInvitationController, PairingProofVerification, PairingSource, SessionEvent,
        SessionState,
    };
    use sonimesh_platform::OutputHealth;
    use sonimesh_protocol::Capability;

    struct HealthyOutput;

    impl OutputSnapshotSource for HealthyOutput {
        fn snapshot(&self) -> OutputSnapshot {
            OutputSnapshot::from_health(false, false, false, OutputHealth::Healthy)
        }
    }

    fn trusted_session() -> SessionState {
        let mut pairing = PairingInvitationController::<1>::new();
        let mut session = SessionState::IDLE;
        pairing
            .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
            .unwrap();
        assert_eq!(
            pairing.verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
                PairingProofVerification::Verified
            }),
            Ok(PairingAttemptOutcome::Paired)
        );
        session
    }

    fn consumed_pairing() -> PairingInvitationController<1> {
        let mut pairing = PairingInvitationController::new();
        let mut session = SessionState::IDLE;
        pairing
            .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
            .unwrap();
        pairing
            .verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
                PairingProofVerification::Verified
            })
            .unwrap();
        pairing
    }

    #[test]
    fn snapshot_projects_no_reconnect_window_outside_grace() {
        let mut coordinator =
            ControlStreamCoordinator::new(trusted_session(), &[Capability::FrameDuration20Ms])
                .unwrap();
        let mut pairing = consumed_pairing();

        let snapshot = coordinator
            .snapshot_at(&mut pairing, 200, &HealthyOutput)
            .unwrap();
        assert_eq!(snapshot.session(), crate::SessionPhase::Ready);
        assert_eq!(snapshot.pairing(), crate::PairingSnapshotState::Consumed);
        assert_eq!(snapshot.reconnect_remaining_ms(), None);
    }

    #[test]
    fn snapshot_advances_and_projects_the_reconnect_window() {
        let mut coordinator =
            ControlStreamCoordinator::new(trusted_session(), &[Capability::FrameDuration20Ms])
                .unwrap();
        let mut pairing = consumed_pairing();

        coordinator
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        coordinator.begin_reconnect_grace(200, 2_000).unwrap();

        let reconnecting = coordinator
            .snapshot_at(&mut pairing, 500, &HealthyOutput)
            .unwrap();
        assert_eq!(reconnecting.session(), crate::SessionPhase::Reconnecting);
        assert_eq!(reconnecting.reconnect_remaining_ms(), Some(1_700));

        let expired = coordinator
            .snapshot_at(&mut pairing, 2_200, &HealthyOutput)
            .unwrap();
        assert_eq!(expired.session(), crate::SessionPhase::Idle);
        assert_eq!(expired.reconnect_remaining_ms(), None);
    }

    #[test]
    fn snapshot_reconnect_clock_regression_fails_closed() {
        let mut coordinator =
            ControlStreamCoordinator::new(trusted_session(), &[Capability::FrameDuration20Ms])
                .unwrap();
        let mut pairing = consumed_pairing();

        coordinator
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        coordinator.begin_reconnect_grace(200, 2_000).unwrap();
        coordinator
            .snapshot_at(&mut pairing, 500, &HealthyOutput)
            .unwrap();

        let regressed = coordinator
            .snapshot_at(&mut pairing, 400, &HealthyOutput)
            .unwrap();
        assert_eq!(regressed.session(), crate::SessionPhase::Idle);
        assert_eq!(regressed.reconnect_remaining_ms(), None);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use sonimesh_protocol::{
        CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello, StartStream,
        StopStream, StreamState, StreamStatus, envelope,
    };

    use super::{
        ControlStreamCoordinator, ControlStreamError, MAX_ACTIVE_STREAMS, MediaStreamIdentity,
        StreamStartRequest,
    };
    use crate::{ControlSessionToken, ControlStreamAction, SessionEvent, SessionState};

    fn coordinator() -> ControlStreamCoordinator {
        ControlStreamCoordinator::new(
            SessionState::READY,
            &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
        )
        .expect("fixed test coordinator is valid")
    }

    fn identity(stream_id: u32) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 0x0102_0304,
            stream_id,
            key_epoch: 0,
        }
    }

    fn request(identity: MediaStreamIdentity) -> StreamStartRequest {
        StreamStartRequest {
            identity,
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration_ms: 10,
        }
    }

    fn frame_capabilities() -> Vec<i32> {
        vec![
            Capability::FrameDuration10Ms as i32,
            Capability::FrameDuration20Ms as i32,
        ]
    }

    fn hello(versions: Vec<u32>, capabilities: Vec<i32>) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 7,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: versions,
                capabilities,
                display_name: "test sender".to_owned(),
            })),
        }
    }

    fn start_envelope(request: StreamStartRequest) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 11,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: request.identity.session_id,
                stream_id: request.identity.stream_id,
                sample_rate_hz: request.sample_rate_hz,
                channels: request.channels,
                frame_duration_ms: request.frame_duration_ms,
                media_key_epoch: u32::from(request.identity.key_epoch),
            })),
        }
    }

    fn stop_envelope(identity: MediaStreamIdentity) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 12,
            payload: Some(envelope::Payload::StopStream(StopStream {
                media_session_id: identity.session_id,
                stream_id: identity.stream_id,
                media_key_epoch: u32::from(identity.key_epoch),
            })),
        }
    }

    fn status_envelope(media_session_id: u64, stream_id: u32) -> Envelope {
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 13,
            payload: Some(envelope::Payload::StreamStatus(StreamStatus {
                media_session_id,
                stream_id,
                state: StreamState::Ready as i32,
                detail: String::new(),
            })),
        }
    }

    fn negotiate(coordinator: &mut ControlStreamCoordinator) -> ControlSessionToken {
        let session = coordinator.install_authenticated_control_session().unwrap();
        match coordinator.accept(
            session,
            &hello(vec![CONTROL_PROTOCOL_VERSION], frame_capabilities()),
        ) {
            Ok(ControlStreamAction::Negotiated(_)) => session,
            other => panic!("expected negotiation, got {other:?}"),
        }
    }

    fn start_stream(
        coordinator: &mut ControlStreamCoordinator,
        session: ControlSessionToken,
        identity: MediaStreamIdentity,
    ) {
        let request = request(identity);
        match coordinator.accept(session, &start_envelope(request)) {
            Ok(ControlStreamAction::Start { token, request }) => {
                coordinator.complete_start(token, request).unwrap();
            }
            other => panic!("expected a start action, got {other:?}"),
        }
    }

    #[test]
    fn install_requires_ready_and_only_once() {
        let mut idle = ControlStreamCoordinator::new(
            SessionState::default(),
            &[Capability::FrameDuration10Ms],
        )
        .unwrap();
        assert!(matches!(
            idle.install_authenticated_control_session(),
            Err(ControlStreamError::ControlSessionRequiresReady(state))
                if state == SessionState::default()
        ));

        let mut ready = coordinator();
        assert!(ready.install_authenticated_control_session().is_ok());
        assert!(matches!(
            ready.install_authenticated_control_session(),
            Err(ControlStreamError::ControlSessionAlreadyInstalled)
        ));
    }

    #[test]
    fn accept_rejects_a_foreign_session_token_before_validation() {
        let mut first = coordinator();
        let first_token = first.install_authenticated_control_session().unwrap();
        let mut second = coordinator();
        let second_token = second.install_authenticated_control_session().unwrap();

        let missing_payload = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 99,
            payload: None,
        };
        // The stale transport check runs before envelope validation, so even a
        // malformed envelope cannot be attributed to a foreign session.
        assert!(matches!(
            first.accept(second_token, &missing_payload),
            Err(ControlStreamError::StaleControlSession)
        ));
        assert!(matches!(
            first.accept(first_token, &missing_payload),
            Err(ControlStreamError::Protocol(
                ControlFrameError::MissingPayload
            ))
        ));
    }

    #[test]
    fn hello_negotiates_the_common_capability_set() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);

        let negotiated = coordinator.negotiation().unwrap();
        assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
        assert!(negotiated.supports(Capability::FrameDuration10Ms));
        assert!(negotiated.supports(Capability::FrameDuration20Ms));

        assert!(matches!(
            coordinator.accept(
                session,
                &hello(vec![CONTROL_PROTOCOL_VERSION], frame_capabilities())
            ),
            Err(ControlStreamError::Renegotiation)
        ));
    }

    #[test]
    fn hello_without_a_common_protocol_version_is_rejected() {
        let mut coordinator = coordinator();
        let session = coordinator.install_authenticated_control_session().unwrap();

        assert!(matches!(
            coordinator.accept(session, &hello(vec![2], frame_capabilities())),
            Err(ControlStreamError::Negotiation(
                sonimesh_protocol::ControlNegotiationError::NoCommonProtocolVersion
            ))
        ));
    }

    #[test]
    fn hello_with_unknown_capabilities_negotiates_an_empty_set() {
        let mut coordinator = coordinator();
        let session = coordinator.install_authenticated_control_session().unwrap();

        assert!(matches!(
            coordinator.accept(session, &hello(vec![CONTROL_PROTOCOL_VERSION], vec![999])),
            Ok(ControlStreamAction::Negotiated(negotiated))
                if negotiated.capabilities().is_empty()
        ));
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(1)))),
            Err(ControlStreamError::UnsupportedFrameDuration(10))
        ));
    }

    #[test]
    fn oversized_hello_capability_set_is_rejected_by_envelope_validation() {
        let mut coordinator = coordinator();
        let session = coordinator.install_authenticated_control_session().unwrap();

        let oversized = hello(
            vec![CONTROL_PROTOCOL_VERSION],
            vec![Capability::ManualAddress as i32; 17],
        );
        assert!(matches!(
            coordinator.accept(session, &oversized),
            Err(ControlStreamError::Protocol(
                ControlFrameError::TooManyCapabilities
            ))
        ));
    }

    #[test]
    fn start_and_stop_require_a_negotiated_hello() {
        let mut coordinator = coordinator();
        let session = coordinator.install_authenticated_control_session().unwrap();

        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(1)))),
            Err(ControlStreamError::MissingNegotiation)
        ));
        assert!(matches!(
            coordinator.accept(session, &stop_envelope(identity(1))),
            Err(ControlStreamError::MissingNegotiation)
        ));
    }

    #[test]
    fn start_requires_a_negotiated_frame_duration() {
        let mut coordinator =
            ControlStreamCoordinator::new(SessionState::READY, &[Capability::FrameDuration20Ms])
                .unwrap();
        let session = coordinator.install_authenticated_control_session().unwrap();
        assert!(matches!(
            coordinator.accept(session, &hello(vec![CONTROL_PROTOCOL_VERSION], vec![2])),
            Ok(ControlStreamAction::Negotiated(_))
        ));

        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(1)))),
            Err(ControlStreamError::UnsupportedFrameDuration(10))
        ));
    }

    #[test]
    fn start_prepares_a_tokenized_action_and_completes_into_streaming() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let request = request(identity(1));

        let ControlStreamAction::Start { token, request } = coordinator
            .accept(session, &start_envelope(request))
            .unwrap()
        else {
            panic!("expected a start action");
        };
        // The action is prepared but the lifecycle does not move until the
        // platform owner acknowledges completion.
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Ready);

        coordinator.complete_start(token, request).unwrap();
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Streaming);
        assert_eq!(coordinator.stream_count(), 1);
        assert_eq!(coordinator.active(), Some(identity(1)));
    }

    #[test]
    fn pending_start_conflicts_are_fail_closed() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let first = request(identity(1));

        let ControlStreamAction::Start { .. } =
            coordinator.accept(session, &start_envelope(first)).unwrap()
        else {
            panic!("expected a start action");
        };

        // An identical pending command is a no-op, a conflicting configuration
        // for the same identity is rejected, and an unrelated identity cannot
        // jump ahead of the pending operation.
        assert!(matches!(
            coordinator.accept(session, &start_envelope(first)),
            Ok(ControlStreamAction::Noop)
        ));
        let mut conflicting = first;
        conflicting.channels = 2;
        assert!(matches!(
            coordinator.accept(session, &start_envelope(conflicting)),
            Err(ControlStreamError::StreamConfigurationConflict)
        ));
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(2)))),
            Err(ControlStreamError::PendingChange)
        ));
    }

    #[test]
    fn completion_mismatch_never_commits() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let first = request(identity(1));
        let second = request(identity(2));

        let ControlStreamAction::Start {
            token: first_token,
            request: first,
        } = coordinator.accept(session, &start_envelope(first)).unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator.complete_start(first_token, first).unwrap();

        let ControlStreamAction::Start {
            token: second_token,
            request: second,
        } = coordinator
            .accept(session, &start_envelope(second))
            .unwrap()
        else {
            panic!("expected a start action");
        };
        // A consumed token cannot complete a different pending operation.
        assert!(matches!(
            coordinator.complete_start(first_token, second),
            Err(ControlStreamError::CompletionMismatch)
        ));
        assert_eq!(coordinator.stream_count(), 1);
        coordinator.complete_start(second_token, second).unwrap();
        assert_eq!(coordinator.stream_count(), 2);
        // A consumed token cannot abort a later completed operation either.
        assert!(matches!(
            coordinator.abort_pending(second_token),
            Err(ControlStreamError::CompletionMismatch)
        ));
    }

    #[test]
    fn abort_pending_clears_only_the_matching_operation() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let request = request(identity(1));

        let ControlStreamAction::Start { token, request } = coordinator
            .accept(session, &start_envelope(request))
            .unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator.abort_pending(token).unwrap();
        // A cleared pending operation cannot be aborted twice, and a fresh
        // command for the same identity is admitted again.
        assert!(matches!(
            coordinator.abort_pending(token),
            Err(ControlStreamError::CompletionMismatch)
        ));
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request)),
            Ok(ControlStreamAction::Start { .. })
        ));
    }

    #[test]
    fn active_stream_limit_fails_closed_at_the_slot_bound() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);

        let slot_bound = u32::try_from(MAX_ACTIVE_STREAMS).expect("slot bound fits in u32");
        for stream_id in 1..=slot_bound {
            start_stream(&mut coordinator, session, identity(stream_id));
        }
        assert_eq!(coordinator.stream_count(), MAX_ACTIVE_STREAMS);
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(9)))),
            Err(ControlStreamError::ActiveStreamLimit)
        ));
    }

    #[test]
    fn stop_commits_and_returns_to_ready_on_the_last_stream() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));
        start_stream(&mut coordinator, session, identity(2));

        let stop = coordinator
            .accept(session, &stop_envelope(identity(1)))
            .unwrap();
        let ControlStreamAction::Stop {
            token,
            identity: stopped_identity,
        } = stop
        else {
            panic!("expected a stop action, got {stop:?}");
        };
        coordinator.complete_stop(token, stopped_identity).unwrap();
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Streaming);
        assert_eq!(coordinator.stream_count(), 1);

        let stop = coordinator
            .accept(session, &stop_envelope(identity(2)))
            .unwrap();
        let ControlStreamAction::Stop {
            token,
            identity: stopped_identity,
        } = stop
        else {
            panic!("expected a stop action, got {stop:?}");
        };
        coordinator.complete_stop(token, stopped_identity).unwrap();
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Ready);
        assert_eq!(coordinator.stream_count(), 0);
    }

    #[test]
    fn stop_of_an_unknown_identity_is_a_noop() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);

        assert!(matches!(
            coordinator.accept(session, &stop_envelope(identity(9))),
            Ok(ControlStreamAction::Noop)
        ));
    }

    #[test]
    fn rotation_replaces_the_identity_in_place_and_rejects_stale_epochs() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let first = request(identity(1));
        let ControlStreamAction::Start {
            token,
            request: first_request,
        } = coordinator.accept(session, &start_envelope(first)).unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator.complete_start(token, first_request).unwrap();

        let mut rotated = request(identity(1));
        rotated.identity.key_epoch = 1;
        let ControlStreamAction::Rotate {
            token,
            previous,
            next,
        } = coordinator
            .accept(session, &start_envelope(rotated))
            .unwrap()
        else {
            panic!("expected a rotate action");
        };
        assert_eq!(previous, identity(1));
        coordinator
            .complete_rotation(token, previous, next)
            .unwrap();
        assert_eq!(coordinator.stream_count(), 1);
        assert_eq!(coordinator.active(), Some(next.identity));

        // The rotated identity is now current: an identical command is a no-op
        // and an older epoch is rejected as stale.
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(next.identity))),
            Ok(ControlStreamAction::Noop)
        ));
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(1)))),
            Err(ControlStreamError::StaleEpoch {
                active: 1,
                requested: 0
            })
        ));
    }

    #[test]
    fn complete_rotation_with_a_stale_token_fails_closed() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let first = request(identity(1));
        let ControlStreamAction::Start {
            token: start_token,
            request: first_request,
        } = coordinator.accept(session, &start_envelope(first)).unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator
            .complete_start(start_token, first_request)
            .unwrap();

        let mut rotated = request(identity(1));
        rotated.identity.key_epoch = 1;
        let ControlStreamAction::Rotate {
            token: rotate_token,
            previous,
            next,
        } = coordinator
            .accept(session, &start_envelope(rotated))
            .unwrap()
        else {
            panic!("expected a rotate action");
        };

        assert!(matches!(
            coordinator.complete_rotation(start_token, previous, next),
            Err(ControlStreamError::CompletionMismatch)
        ));
        // The pending rotation survives a rejected completion, so the matching
        // token still commits it.
        coordinator
            .complete_rotation(rotate_token, previous, next)
            .unwrap();
        assert_eq!(coordinator.active(), Some(next.identity));
    }

    #[test]
    fn output_lost_clears_pending_and_blocks_late_commits() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));

        let second = request(identity(2));
        let ControlStreamAction::Start {
            token,
            request: second_request,
        } = coordinator
            .accept(session, &start_envelope(second))
            .unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator
            .acknowledge_platform_event(SessionEvent::OutputLost)
            .unwrap();
        assert_eq!(
            coordinator.state().phase(),
            crate::SessionPhase::OutputUnavailable
        );

        assert!(matches!(
            coordinator.complete_start(token, second_request),
            Err(ControlStreamError::CompletionMismatch)
        ));
        assert!(matches!(
            coordinator.accept(session, &start_envelope(request(identity(3)))),
            Err(ControlStreamError::StartRequiresStreaming(
                SessionState::OUTPUT_UNAVAILABLE
            ))
        ));
    }

    #[test]
    fn output_restored_returns_to_streaming_without_dropping_active_streams() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));

        coordinator
            .acknowledge_platform_event(SessionEvent::OutputLost)
            .unwrap();
        coordinator
            .acknowledge_platform_event(SessionEvent::OutputRestored)
            .unwrap();
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Streaming);
        assert_eq!(coordinator.stream_count(), 1);
        assert_eq!(coordinator.active(), Some(identity(1)));
    }

    #[test]
    fn output_lost_from_ready_is_an_invalid_transition() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        let _ = session;

        assert!(matches!(
            coordinator.acknowledge_platform_event(SessionEvent::OutputLost),
            Err(ControlStreamError::Transition(_))
        ));
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Ready);
    }

    #[test]
    fn connection_lost_retires_the_transport_and_pending_work() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));

        let second = request(identity(2));
        let ControlStreamAction::Start { token, request } = coordinator
            .accept(session, &start_envelope(second))
            .unwrap()
        else {
            panic!("expected a start action");
        };
        coordinator
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        assert_eq!(
            coordinator.state().phase(),
            crate::SessionPhase::Reconnecting
        );

        // The lost transport invalidates the pending operation and the session
        // token; negotiation survives until a reconnect replaces it.
        assert!(matches!(
            coordinator.complete_start(token, request),
            Err(ControlStreamError::CompletionMismatch)
        ));
        assert!(matches!(
            coordinator.accept(
                session,
                &hello(vec![CONTROL_PROTOCOL_VERSION], frame_capabilities())
            ),
            Err(ControlStreamError::StaleControlSession)
        ));
        // A fresh transport cannot be installed until the lifecycle returns to
        // Ready; a reconnecting session still rejects it.
        assert!(matches!(
            coordinator.install_authenticated_control_session(),
            Err(ControlStreamError::ControlSessionRequiresReady(
                SessionState::RECONNECTING
            ))
        ));

        // Reconnecting with fresh material retires the old negotiation, so a
        // reinstalled transport can negotiate a new capability set.
        coordinator
            .acknowledge_platform_event(SessionEvent::Reconnected)
            .unwrap();
        let fresh = coordinator.install_authenticated_control_session().unwrap();
        assert!(matches!(
            coordinator.accept(
                fresh,
                &hello(vec![CONTROL_PROTOCOL_VERSION], frame_capabilities())
            ),
            Ok(ControlStreamAction::Negotiated(_))
        ));
    }

    #[test]
    fn reconnected_clears_streams_and_allows_a_fresh_negotiation() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));
        coordinator
            .acknowledge_platform_event(SessionEvent::ConnectionLost)
            .unwrap();
        coordinator
            .acknowledge_platform_event(SessionEvent::Reconnected)
            .unwrap();

        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Ready);
        assert_eq!(coordinator.stream_count(), 0);
        assert!(coordinator.negotiation().is_none());

        let fresh = negotiate(&mut coordinator);
        start_stream(&mut coordinator, fresh, identity(3));
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Streaming);
    }

    #[test]
    fn unsupported_lifecycle_events_are_rejected() {
        let mut coordinator = coordinator();
        for event in [
            SessionEvent::StartPairing,
            SessionEvent::StartStream,
            SessionEvent::StopStream,
            SessionEvent::SessionExpired,
        ] {
            assert!(matches!(
                coordinator.acknowledge_platform_event(event),
                Err(ControlStreamError::UnsupportedLifecycleEvent(found))
                    if found == event
            ));
        }
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Ready);
    }

    #[test]
    fn expire_control_session_retires_every_session_owned_value() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);
        start_stream(&mut coordinator, session, identity(1));

        coordinator.expire_control_session();
        assert_eq!(coordinator.state().phase(), crate::SessionPhase::Idle);
        assert_eq!(coordinator.stream_count(), 0);
        assert!(coordinator.negotiation().is_none());
        assert!(matches!(
            coordinator.accept(
                session,
                &hello(vec![CONTROL_PROTOCOL_VERSION], frame_capabilities())
            ),
            Err(ControlStreamError::StaleControlSession)
        ));
    }

    #[test]
    fn stream_status_payloads_are_ignored_after_validation() {
        let mut coordinator = coordinator();
        let session = negotiate(&mut coordinator);

        assert!(matches!(
            coordinator.accept(session, &status_envelope(1, 1)),
            Ok(ControlStreamAction::Ignored)
        ));
        assert!(matches!(
            coordinator.accept(session, &status_envelope(0, 1)),
            Err(ControlStreamError::Protocol(
                ControlFrameError::InvalidStreamStatusIdentity
            ))
        ));
    }
}
