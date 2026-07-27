//! Low-rate control lifecycle tests independent of output and network adapters.

use core::ops::{Deref, DerefMut};

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, ControlStreamError,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingInvitationError,
    PairingProofVerification, PairingRecordError, PairingRecordState, PairingSource, SessionEvent,
    SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello,
    MAX_CONTROL_CAPABILITIES, MediaDirection, StartStream, StopStream, envelope,
};

struct AuthenticatedCoordinator {
    inner: ControlStreamCoordinator,
    session: ControlSessionToken,
}

impl AuthenticatedCoordinator {
    fn from_ready(
        state: SessionState,
        capabilities: &[Capability],
    ) -> Result<Self, ControlStreamError> {
        Self::install(ControlStreamCoordinator::new(state, capabilities)?)
    }

    fn install(mut inner: ControlStreamCoordinator) -> Result<Self, ControlStreamError> {
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

fn start(epoch: u32) -> Envelope {
    start_with_duration(epoch, 20)
}

fn start_with_duration(epoch: u32, frame_duration_ms: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) + 1,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms,
            media_key_epoch: epoch,
        })),
    }
}

fn stop(epoch: u32) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch) + 100,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: 7,
            stream_id: 9,
            media_key_epoch: epoch,
        })),
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

fn hello(capabilities: &[Capability]) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: capabilities
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn coordinator(capabilities: &[Capability]) -> AuthenticatedCoordinator {
    let mut coordinator =
        AuthenticatedCoordinator::from_ready(trusted_session(), capabilities).unwrap();
    assert!(matches!(
        coordinator.accept(&hello(capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    coordinator
}

fn pair_and_negotiate(
    coordinator: &mut AuthenticatedCoordinator,
    capabilities: &[Capability],
    id_byte: u8,
) {
    let mut pairing = PairingInvitationController::<1>::new();
    coordinator
        .begin_pairing(
            &mut pairing,
            PairingId::new([id_byte; 16]),
            u64::from(id_byte) * 100,
            50,
            1,
        )
        .unwrap();
    assert_eq!(
        coordinator.verify_pairing_proof(
            &mut pairing,
            u64::from(id_byte) * 100 + 1,
            PairingSource::new([id_byte.wrapping_add(1); 16]),
            || PairingProofVerification::Verified,
        ),
        Ok(PairingAttemptOutcome::Paired)
    );
    coordinator.install_session().unwrap();
    assert!(matches!(
        coordinator.accept(&hello(capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

fn activate(coordinator: &mut AuthenticatedCoordinator, epoch: u32) {
    let ControlStreamAction::Start { token, request } = coordinator.accept(&start(epoch)).unwrap()
    else {
        panic!("expected a stream start action");
    };
    coordinator.complete_start(token, request).unwrap();
}

#[test]
fn start_completion_advances_lifecycle_only_after_platform_success() {
    let mut coordinator =
        coordinator(&[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms]);
    let ControlStreamAction::Start { token, request } = coordinator.accept(&start(0)).unwrap()
    else {
        panic!("expected a stream start action");
    };

    assert_eq!(coordinator.active(), None);
    assert_eq!(
        coordinator.accept(&start(0)).unwrap(),
        ControlStreamAction::Noop
    );
    assert_eq!(
        coordinator.state().phase(),
        sonimesh_app::SessionPhase::Ready
    );
    coordinator.complete_start(token, request).unwrap();
    assert_eq!(coordinator.active(), Some(request.identity));
    assert_eq!(
        coordinator.state().phase(),
        sonimesh_app::SessionPhase::Streaming
    );
    assert!(matches!(
        coordinator.complete_start(token, request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    assert_eq!(
        request
            .identity
            .media_key_context(MediaDirection::AndroidToDesktop)
            .unwrap()
            .key_epoch,
        0
    );
}

#[test]
fn rotates_only_to_a_higher_epoch_and_rejects_stale_start() {
    let mut coordinator =
        coordinator(&[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms]);
    activate(&mut coordinator, 2);
    let previous = coordinator.active().unwrap();

    let ControlStreamAction::Rotate {
        token,
        previous: from,
        next,
    } = coordinator.accept(&start(3)).unwrap()
    else {
        panic!("expected a key rotation action");
    };
    assert_eq!(from, previous);
    assert_eq!(coordinator.active(), Some(previous));
    coordinator.complete_rotation(token, from, next).unwrap();
    assert_eq!(coordinator.active(), Some(next.identity));
    assert!(matches!(
        coordinator.accept(&start(2)),
        Err(ControlStreamError::StaleEpoch {
            active: 3,
            requested: 2,
        })
    ));
}

#[test]
fn stale_stop_cannot_stop_the_newer_active_epoch() {
    let mut coordinator =
        coordinator(&[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms]);
    activate(&mut coordinator, 4);
    let active = coordinator.active().unwrap();

    assert_eq!(
        coordinator.accept(&stop(3)).unwrap(),
        ControlStreamAction::Noop
    );
    assert_eq!(coordinator.active(), Some(active));
    let ControlStreamAction::Stop { token, identity } = coordinator.accept(&stop(4)).unwrap()
    else {
        panic!("expected a stream stop action");
    };
    assert_eq!(identity, active);
    coordinator.complete_stop(token, identity).unwrap();
    assert_eq!(coordinator.active(), None);
}

#[test]
fn invalid_control_frame_cannot_prepare_media_work() {
    let mut coordinator =
        coordinator(&[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms]);
    let invalid = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: u32::from(u16::MAX) + 1,
        })),
    };

    assert!(matches!(
        coordinator.accept(&invalid),
        Err(ControlStreamError::Protocol(
            ControlFrameError::InvalidMediaKeyEpoch
        ))
    ));
    assert_eq!(coordinator.active(), None);
}

#[test]
fn rejects_frame_duration_not_selected_by_hello_negotiation() {
    let mut coordinator = coordinator(&[Capability::FrameDuration20Ms]);
    let mut command = start(0);
    let Some(envelope::Payload::StartStream(stream_start)) = command.payload.as_mut() else {
        panic!("expected start command");
    };
    stream_start.frame_duration_ms = 10;

    assert!(matches!(
        coordinator.accept(&command),
        Err(ControlStreamError::UnsupportedFrameDuration(10))
    ));
    assert_eq!(coordinator.active(), None);
    assert!(matches!(
        coordinator.accept(&start(1)),
        Ok(ControlStreamAction::Start { .. })
    ));
}

#[test]
fn accepts_selected_10_ms_start_and_rotation() {
    let mut coordinator =
        coordinator(&[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms]);
    let ControlStreamAction::Start {
        token: initial_token,
        request: initial,
    } = coordinator.accept(&start_with_duration(2, 10)).unwrap()
    else {
        panic!("expected 10 ms start action");
    };
    coordinator.complete_start(initial_token, initial).unwrap();

    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = coordinator.accept(&start_with_duration(3, 10)).unwrap()
    else {
        panic!("expected 10 ms rotation action");
    };
    coordinator
        .complete_rotation(token, previous, next)
        .unwrap();
    assert_eq!(coordinator.active(), Some(next.identity));
}

#[test]
fn start_requires_hello_and_coordinator_rejects_renegotiation() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator =
        AuthenticatedCoordinator::from_ready(trusted_session(), &capabilities).unwrap();

    assert!(matches!(
        coordinator.accept(&start(0)),
        Err(ControlStreamError::MissingNegotiation)
    ));
    assert!(matches!(
        coordinator.accept(&hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    assert!(matches!(
        coordinator.accept(&hello(&capabilities)),
        Err(ControlStreamError::Renegotiation)
    ));
    assert!(matches!(
        coordinator.accept(&start(0)),
        Ok(ControlStreamAction::Start { .. })
    ));
}

#[test]
fn authenticated_control_session_requires_trusted_ready_state() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = ControlStreamCoordinator::new(SessionState::IDLE, &capabilities).unwrap();

    assert!(matches!(
        coordinator.install_authenticated_control_session(),
        Err(ControlStreamError::ControlSessionRequiresReady(
            SessionState::IDLE
        ))
    ));
    assert_eq!(coordinator.negotiation(), None);
}

#[test]
fn authenticated_transport_tokens_do_not_cross_coordinators() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut first = ControlStreamCoordinator::new(trusted_session(), &capabilities).unwrap();
    let mut second = ControlStreamCoordinator::new(trusted_session(), &capabilities).unwrap();
    let first_session = first.install_authenticated_control_session().unwrap();
    let second_session = second.install_authenticated_control_session().unwrap();

    assert_ne!(first_session, second_session);
    assert!(matches!(
        second.accept(first_session, &hello(&capabilities)),
        Err(ControlStreamError::StaleControlSession)
    ));
    assert!(matches!(
        second.accept(second_session, &hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn operation_tokens_do_not_cross_coordinators_with_identical_pending_work() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut first = coordinator(&capabilities);
    let mut second = coordinator(&capabilities);
    let ControlStreamAction::Start {
        token: first_token,
        request: first_request,
    } = first.accept(&start(0)).unwrap()
    else {
        panic!("expected first pending start");
    };
    let ControlStreamAction::Start {
        token: second_token,
        request: second_request,
    } = second.accept(&start(0)).unwrap()
    else {
        panic!("expected second pending start");
    };

    assert_eq!(first_request, second_request);
    assert_ne!(first_token, second_token);
    assert!(matches!(
        first.complete_start(second_token, second_request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    first.complete_start(first_token, first_request).unwrap();
    second.complete_start(second_token, second_request).unwrap();
}

#[test]
fn connection_loss_retires_the_transport_token_and_pending_work() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), &capabilities).unwrap();
    let retired_session = coordinator.install_authenticated_control_session().unwrap();

    assert!(matches!(
        coordinator.install_authenticated_control_session(),
        Err(ControlStreamError::ControlSessionAlreadyInstalled)
    ));
    assert!(matches!(
        coordinator.accept(retired_session, &hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    let ControlStreamAction::Start { token, request } =
        coordinator.accept(retired_session, &start(0)).unwrap()
    else {
        panic!("expected pending start action");
    };

    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    assert!(matches!(
        coordinator.complete_start(token, request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    let malformed = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION + 1,
        request_id: 99,
        payload: None,
    };
    assert!(matches!(
        coordinator.accept(retired_session, &malformed),
        Err(ControlStreamError::StaleControlSession)
    ));

    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .unwrap();
    let fresh_session = coordinator.install_authenticated_control_session().unwrap();
    assert_ne!(retired_session, fresh_session);
    assert!(matches!(
        coordinator.accept(retired_session, &hello(&capabilities)),
        Err(ControlStreamError::StaleControlSession)
    ));
    assert!(matches!(
        coordinator.accept(fresh_session, &hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn rejects_oversized_local_capability_policy() {
    let oversized = vec![Capability::ManualAddress; MAX_CONTROL_CAPABILITIES + 1];
    assert!(matches!(
        ControlStreamCoordinator::new(SessionState::IDLE, &oversized),
        Err(ControlStreamError::LocalCapabilityPolicyTooLarge)
    ));
}

#[test]
fn failed_hello_keeps_start_blocked_until_a_valid_intersection_is_selected() {
    let capabilities = [Capability::FrameDuration10Ms, Capability::FrameDuration20Ms];
    let mut coordinator =
        AuthenticatedCoordinator::from_ready(trusted_session(), &capabilities).unwrap();
    let incompatible = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![2],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "Lab sender".to_owned(),
        })),
    };

    assert!(matches!(
        coordinator.accept(&incompatible),
        Err(ControlStreamError::Negotiation(_))
    ));
    assert_eq!(coordinator.negotiation(), None);
    assert!(matches!(
        coordinator.accept(&start(0)),
        Err(ControlStreamError::MissingNegotiation)
    ));
    assert!(matches!(
        coordinator.accept(&hello(&[Capability::FrameDuration20Ms])),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    assert!(matches!(
        coordinator.accept(&start_with_duration(0, 10)),
        Err(ControlStreamError::UnsupportedFrameDuration(10))
    ));
    assert!(matches!(
        coordinator.accept(&start(0)),
        Ok(ControlStreamAction::Start { .. })
    ));
}

#[test]
fn expiring_control_session_retires_stream_and_allows_fresh_hello() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    activate(&mut coordinator, 2);
    assert!(coordinator.active().is_some());

    coordinator.expire_control_session();
    assert_eq!(coordinator.state(), SessionState::IDLE);
    assert_eq!(coordinator.active(), None);
    assert_eq!(coordinator.negotiation(), None);

    let mut pairing = PairingInvitationController::<1>::new();
    coordinator
        .begin_pairing(&mut pairing, PairingId::new([3; 16]), 200, 50, 1)
        .unwrap();
    assert_eq!(
        coordinator.verify_pairing_proof(&mut pairing, 201, PairingSource::new([4; 16]), || {
            PairingProofVerification::Verified
        },),
        Ok(PairingAttemptOutcome::Paired)
    );
    coordinator.install_session().unwrap();
    assert!(matches!(
        coordinator.accept(&hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn stale_start_completion_cannot_commit_an_identical_fresh_session_request() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    let ControlStreamAction::Start {
        token: stale_token,
        request: stale_request,
    } = coordinator.accept(&start(0)).unwrap()
    else {
        panic!("expected initial start action");
    };

    coordinator.expire_control_session();
    let mut pairing = PairingInvitationController::<1>::new();
    coordinator
        .begin_pairing(&mut pairing, PairingId::new([8; 16]), 300, 50, 1)
        .unwrap();
    assert_eq!(
        coordinator.verify_pairing_proof(&mut pairing, 301, PairingSource::new([9; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    coordinator.install_session().unwrap();
    assert!(matches!(
        coordinator.accept(&hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    let ControlStreamAction::Start {
        token: fresh_token,
        request: fresh_request,
    } = coordinator.accept(&start(0)).unwrap()
    else {
        panic!("expected fresh start action");
    };

    assert_ne!(stale_token, fresh_token);
    assert_eq!(stale_request, fresh_request);
    assert!(matches!(
        coordinator.complete_start(stale_token, stale_request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    assert_eq!(
        coordinator.state().phase(),
        sonimesh_app::SessionPhase::Ready
    );
    coordinator
        .complete_start(fresh_token, fresh_request)
        .unwrap();
    assert_eq!(coordinator.active(), Some(fresh_request.identity));
}

#[test]
fn stale_rotation_completion_cannot_commit_an_identical_fresh_session_request() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    activate(&mut coordinator, 0);
    let ControlStreamAction::Rotate {
        token: stale_token,
        previous: stale_previous,
        next: stale_next,
    } = coordinator.accept(&start(1)).unwrap()
    else {
        panic!("expected initial rotation action");
    };

    coordinator.expire_control_session();
    pair_and_negotiate(&mut coordinator, &capabilities, 10);
    activate(&mut coordinator, 0);
    let ControlStreamAction::Rotate {
        token: fresh_token,
        previous: fresh_previous,
        next: fresh_next,
    } = coordinator.accept(&start(1)).unwrap()
    else {
        panic!("expected fresh rotation action");
    };

    assert_ne!(stale_token, fresh_token);
    assert_eq!(stale_previous, fresh_previous);
    assert_eq!(stale_next, fresh_next);
    assert!(matches!(
        coordinator.complete_rotation(stale_token, stale_previous, stale_next),
        Err(ControlStreamError::CompletionMismatch)
    ));
    coordinator
        .complete_rotation(fresh_token, fresh_previous, fresh_next)
        .unwrap();
    assert_eq!(coordinator.active(), Some(fresh_next.identity));
}

#[test]
fn output_loss_invalidates_a_pending_rotation_before_recovery() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    activate(&mut coordinator, 0);
    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = coordinator.accept(&start(1)).unwrap()
    else {
        panic!("expected rotation action");
    };

    coordinator
        .acknowledge_platform_event(SessionEvent::OutputLost)
        .unwrap();
    assert_eq!(
        coordinator.state().phase(),
        sonimesh_app::SessionPhase::OutputUnavailable
    );
    coordinator
        .acknowledge_platform_event(SessionEvent::OutputRestored)
        .unwrap();

    assert!(matches!(
        coordinator.complete_rotation(token, previous, next),
        Err(ControlStreamError::CompletionMismatch)
    ));
    assert_eq!(coordinator.active(), Some(previous));
    assert!(matches!(
        coordinator.accept(&start(1)),
        Ok(ControlStreamAction::Rotate { .. })
    ));
}

#[test]
fn stale_stop_completion_cannot_stop_an_identical_fresh_session_stream() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    activate(&mut coordinator, 0);
    let ControlStreamAction::Stop {
        token: stale_token,
        identity: stale_identity,
    } = coordinator.accept(&stop(0)).unwrap()
    else {
        panic!("expected initial stop action");
    };

    coordinator.expire_control_session();
    pair_and_negotiate(&mut coordinator, &capabilities, 12);
    activate(&mut coordinator, 0);
    let ControlStreamAction::Stop {
        token: fresh_token,
        identity: fresh_identity,
    } = coordinator.accept(&stop(0)).unwrap()
    else {
        panic!("expected fresh stop action");
    };

    assert_ne!(stale_token, fresh_token);
    assert_eq!(stale_identity, fresh_identity);
    assert!(matches!(
        coordinator.complete_stop(stale_token, stale_identity),
        Err(ControlStreamError::CompletionMismatch)
    ));
    coordinator
        .complete_stop(fresh_token, fresh_identity)
        .unwrap();
    assert_eq!(coordinator.active(), None);
}

#[test]
fn platform_acknowledgements_cannot_expire_a_control_session() {
    let mut coordinator = coordinator(&[Capability::FrameDuration20Ms]);
    assert!(matches!(
        coordinator.acknowledge_platform_event(SessionEvent::SessionExpired),
        Err(ControlStreamError::UnsupportedLifecycleEvent(
            SessionEvent::SessionExpired
        ))
    ));
    assert!(coordinator.negotiation().is_some());
}

#[test]
fn reconnect_with_fresh_material_invalidates_pending_start_and_negotiation() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = coordinator(&capabilities);
    let ControlStreamAction::Start {
        token: stale_token,
        request: stale_request,
    } = coordinator.accept(&start(0)).unwrap()
    else {
        panic!("expected start action");
    };

    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .unwrap();
    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .unwrap();

    assert_eq!(coordinator.negotiation(), None);
    assert_eq!(coordinator.active(), None);
    assert!(matches!(
        coordinator.complete_start(stale_token, stale_request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    assert!(matches!(
        coordinator.accept(&start(0)),
        Err(ControlStreamError::StaleControlSession)
    ));
    coordinator.install_session().unwrap();
    assert!(matches!(
        coordinator.accept(&start(0)),
        Err(ControlStreamError::MissingNegotiation)
    ));
    assert!(matches!(
        coordinator.accept(&hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn expiring_active_pairing_returns_the_coordinator_to_idle() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = ControlStreamCoordinator::new(SessionState::IDLE, &capabilities).unwrap();
    let mut pairing = PairingInvitationController::<1>::new();
    coordinator
        .begin_pairing(&mut pairing, PairingId::new([5; 16]), 400, 50, 1)
        .unwrap();

    coordinator.expire_pairing(&mut pairing).unwrap();
    assert_eq!(coordinator.state(), SessionState::IDLE);
    assert_eq!(pairing.state(), Some(PairingRecordState::Expired));
    assert_eq!(coordinator.active(), None);
    assert_eq!(coordinator.negotiation(), None);
}

#[test]
fn revocation_retires_the_trusted_control_session_and_stream() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut trusted_pairing = PairingInvitationController::<1>::new();
    let mut trusted_coordinator =
        ControlStreamCoordinator::new(SessionState::IDLE, &capabilities).unwrap();
    trusted_coordinator
        .begin_pairing(&mut trusted_pairing, PairingId::new([10; 16]), 500, 50, 1)
        .unwrap();
    assert_eq!(
        trusted_coordinator.verify_pairing_proof(
            &mut trusted_pairing,
            501,
            PairingSource::new([11; 16]),
            || PairingProofVerification::Verified,
        ),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut trusted_coordinator = AuthenticatedCoordinator::install(trusted_coordinator).unwrap();
    assert!(matches!(
        trusted_coordinator.accept(&hello(&capabilities)),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    activate(&mut trusted_coordinator, 0);
    let ControlStreamAction::Rotate {
        token: pending_token,
        previous,
        next,
    } = trusted_coordinator.accept(&start(1)).unwrap()
    else {
        panic!("expected a pending rotation");
    };

    assert_eq!(
        trusted_coordinator.revoke_pairing(&mut trusted_pairing),
        Ok(PairingRecordState::Revoked)
    );
    assert_eq!(trusted_coordinator.state(), SessionState::IDLE);
    assert_eq!(trusted_coordinator.active(), None);
    assert_eq!(trusted_coordinator.negotiation(), None);
    assert!(matches!(
        trusted_coordinator.complete_rotation(pending_token, previous, next),
        Err(ControlStreamError::CompletionMismatch)
    ));
    assert!(matches!(
        trusted_coordinator.accept(&hello(&capabilities)),
        Err(ControlStreamError::StaleControlSession)
    ));
}

#[test]
fn observed_pairing_failure_retires_the_coordinator_and_rejects_control_frames() {
    let capabilities = [Capability::FrameDuration20Ms];
    let mut coordinator = ControlStreamCoordinator::new(SessionState::IDLE, &capabilities).unwrap();
    let mut pairing = PairingInvitationController::<1>::new();
    coordinator
        .begin_pairing(&mut pairing, PairingId::new([6; 16]), 400, 50, 1)
        .unwrap();

    assert_eq!(
        coordinator.observe_pairing(&mut pairing, 399),
        Err(PairingInvitationError::Record(
            PairingRecordError::TimeRegressed
        ))
    );
    assert_eq!(coordinator.state(), SessionState::IDLE);
    assert_eq!(pairing.state(), Some(PairingRecordState::Expired));
    assert_eq!(coordinator.active(), None);
    assert_eq!(coordinator.negotiation(), None);
    assert!(matches!(
        coordinator.install_authenticated_control_session(),
        Err(ControlStreamError::ControlSessionRequiresReady(
            SessionState::IDLE
        ))
    ));

    coordinator
        .begin_pairing(&mut pairing, PairingId::new([7; 16]), 500, 50, 1)
        .unwrap();
    assert_eq!(
        coordinator.observe_pairing(&mut pairing, 550),
        Ok(PairingRecordState::Expired)
    );
    assert_eq!(coordinator.state(), SessionState::IDLE);
    assert_eq!(coordinator.active(), None);
    assert_eq!(coordinator.negotiation(), None);
}
