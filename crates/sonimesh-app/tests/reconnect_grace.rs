//! Control-session reconnect-grace lifecycle independent of output and network.

use core::ops::{Deref, DerefMut};

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, ControlStreamError,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingSource, ReconnectGraceError, ReconnectGraceObservation,
    ReconnectGraceState, SessionEvent, SessionPhase, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, StartStream, envelope,
};

const CAPABILITIES: &[Capability] = &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms];

struct AuthenticatedCoordinator {
    inner: ControlStreamCoordinator,
    session: ControlSessionToken,
}

impl AuthenticatedCoordinator {
    fn from_ready(state: SessionState) -> Result<Self, ControlStreamError> {
        let mut inner = ControlStreamCoordinator::new(state, CAPABILITIES)?;
        let session = inner.install_authenticated_control_session()?;
        Ok(Self { inner, session })
    }

    fn install_session(&mut self) -> Result<ControlSessionToken, ControlStreamError> {
        let session = self.inner.install_authenticated_control_session()?;
        self.session = session;
        Ok(session)
    }

    fn accept(&mut self, envelope: &Envelope) -> Result<ControlStreamAction, ControlStreamError> {
        self.inner.accept(self.session, envelope)
    }
}

impl Deref for AuthenticatedCoordinator {
    type Target = ControlStreamCoordinator;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for AuthenticatedCoordinator {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
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

fn hello() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: CAPABILITIES
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn start(epoch: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) + 1,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: epoch,
        })),
    }
}

/// Builds a negotiated, streaming coordinator through the trusted control path.
fn streaming_coordinator() -> AuthenticatedCoordinator {
    let mut coordinator = AuthenticatedCoordinator::from_ready(trusted_session()).unwrap();
    assert!(matches!(
        coordinator.accept(&hello()),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    let ControlStreamAction::Start { token, request } = coordinator.accept(&start(1)).unwrap()
    else {
        panic!("expected a stream start action");
    };
    coordinator.complete_start(token, request).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);
    coordinator
}

#[test]
fn connection_loss_arms_grace_and_expiry_retires_the_whole_session() {
    let mut coordinator = streaming_coordinator();

    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Reconnecting);
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );

    coordinator.begin_reconnect_grace(1_000, 5_000).unwrap();
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::InGrace
    );
    assert!(matches!(
        coordinator.observe_reconnect_grace(1_000),
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: 5_000
        })
    ));
    assert!(matches!(
        coordinator.observe_reconnect_grace(4_000),
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: 2_000
        })
    ));
    assert_eq!(coordinator.state().phase(), SessionPhase::Reconnecting);

    assert!(matches!(
        coordinator.observe_reconnect_grace(6_000),
        Ok(ReconnectGraceObservation::Expired)
    ));
    assert_eq!(coordinator.state().phase(), SessionPhase::Idle);
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );
    assert_eq!(coordinator.active(), None);
    assert_eq!(coordinator.negotiation(), None);

    // The retired control transport cannot accept a delayed envelope.
    assert!(matches!(
        coordinator.accept(&start(1)),
        Err(ControlStreamError::StaleControlSession)
    ));

    // Grace cannot be re-armed outside the reconnecting lifecycle.
    assert!(matches!(
        coordinator.begin_reconnect_grace(6_001, 1_000),
        Err(ControlStreamError::GraceRequiresReconnecting(state)) if state.phase() == SessionPhase::Idle
    ));
}

#[test]
fn reconnect_before_the_deadline_keeps_the_ready_session() {
    let mut coordinator = streaming_coordinator();

    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator.begin_reconnect_grace(1_000, 5_000).unwrap();
    assert!(matches!(
        coordinator.observe_reconnect_grace(5_000),
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: 1_000
        })
    ));

    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Ready);
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );

    // Fresh session material can be installed and negotiated again.
    coordinator.install_session().unwrap();
    assert!(matches!(
        coordinator.accept(&hello()),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn grace_requires_the_reconnecting_lifecycle() {
    let mut ready = AuthenticatedCoordinator::from_ready(trusted_session()).unwrap();
    assert!(matches!(
        ready.begin_reconnect_grace(1_000, 1_000),
        Err(ControlStreamError::GraceRequiresReconnecting(state)) if state.phase() == SessionPhase::Ready
    ));

    let mut streaming = streaming_coordinator();
    assert!(matches!(
        streaming.begin_reconnect_grace(1_000, 1_000),
        Err(ControlStreamError::GraceRequiresReconnecting(state)) if state.phase() == SessionPhase::Streaming
    ));
}

#[test]
fn grace_clock_regression_expires_the_session_fail_closed() {
    let mut coordinator = streaming_coordinator();
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator.begin_reconnect_grace(1_000, 5_000).unwrap();
    assert!(matches!(
        coordinator.observe_reconnect_grace(2_000),
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: 4_000
        })
    ));

    assert!(matches!(
        coordinator.observe_reconnect_grace(1_500),
        Err(ControlStreamError::ReconnectGrace(
            ReconnectGraceError::TimeRegressed
        ))
    ));
    assert_eq!(coordinator.state().phase(), SessionPhase::Idle);
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );
}

#[test]
fn repeated_connection_loss_cannot_rearm_an_active_window() {
    let mut coordinator = streaming_coordinator();
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator.begin_reconnect_grace(1_000, 5_000).unwrap();

    assert!(matches!(
        coordinator.begin_reconnect_grace(1_100, 5_000),
        Err(ControlStreamError::ReconnectGrace(
            ReconnectGraceError::AlreadyInGrace
        ))
    ));
    // The lifecycle still rejects a second loss while reconnecting.
    assert!(matches!(
        coordinator.acknowledge_platform_event(SessionEvent::ConnectionLost),
        Err(ControlStreamError::Transition(_))
    ));
    assert_eq!(coordinator.state().phase(), SessionPhase::Reconnecting);
}

#[test]
fn explicit_session_expiry_retires_an_armed_grace_window() {
    let mut coordinator = streaming_coordinator();
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator.begin_reconnect_grace(1_000, 5_000).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Reconnecting);

    coordinator.expire_control_session();
    assert_eq!(coordinator.state().phase(), SessionPhase::Idle);
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );
}

#[test]
fn idle_grace_stays_idle_through_pairing_owner_observations() {
    let mut coordinator = AuthenticatedCoordinator::from_ready(trusted_session()).unwrap();
    let mut pairing = PairingInvitationController::<1>::new();
    assert!(matches!(
        coordinator.observe_pairing(&mut pairing, 1_000),
        Err(PairingInvitationError::MissingInvitation)
    ));
    assert_eq!(
        coordinator.reconnect_grace_state(),
        ReconnectGraceState::Idle
    );
}

#[test]
fn grace_window_survives_until_the_deadline_inclusive() {
    let mut coordinator = streaming_coordinator();
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator.begin_reconnect_grace(1_000, 100).unwrap();

    assert!(matches!(
        coordinator.observe_reconnect_grace(1_099),
        Ok(ReconnectGraceObservation::InGrace { remaining_ms: 1 })
    ));
    assert!(matches!(
        coordinator.observe_reconnect_grace(1_100),
        Ok(ReconnectGraceObservation::Expired)
    ));
}
