//! Low-rate control lifecycle tests independent of output and network adapters.

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, ControlStreamError, PairingAttemptOutcome,
    PairingId, PairingInvitationController, PairingSource, SessionEvent, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello, MediaDirection,
    StartStream, StopStream, envelope, negotiate_hello,
};

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
        pairing.record_proof(&mut session, 101, PairingSource::new([2; 16]), true),
        Ok(PairingAttemptOutcome::Paired)
    );
    session
}

fn negotiation() -> sonimesh_protocol::NegotiatedControl {
    negotiate_hello(
        &Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![
                Capability::FrameDuration10Ms as i32,
                Capability::FrameDuration20Ms as i32,
            ],
            display_name: "Lab sender".to_owned(),
        },
        &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
    )
    .unwrap()
}

fn activate(coordinator: &mut ControlStreamCoordinator, epoch: u32) {
    let ControlStreamAction::Start(request) = coordinator.accept(&start(epoch)).unwrap() else {
        panic!("expected a stream start action");
    };
    coordinator
        .state_mut()
        .apply(SessionEvent::StartStream)
        .unwrap();
    coordinator.complete_start(request).unwrap();
}

#[test]
fn starts_only_after_platform_state_reports_streaming() {
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiation());
    let ControlStreamAction::Start(request) = coordinator.accept(&start(0)).unwrap() else {
        panic!("expected a stream start action");
    };

    assert_eq!(coordinator.active(), None);
    assert_eq!(
        coordinator.accept(&start(0)).unwrap(),
        ControlStreamAction::Noop
    );
    assert!(matches!(
        coordinator.complete_start(request),
        Err(ControlStreamError::CompletionMismatch)
    ));
    coordinator
        .state_mut()
        .apply(SessionEvent::StartStream)
        .unwrap();
    coordinator.complete_start(request).unwrap();
    assert_eq!(coordinator.active(), Some(request.identity));
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
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiation());
    activate(&mut coordinator, 2);
    let previous = coordinator.active().unwrap();

    let ControlStreamAction::Rotate {
        previous: from,
        next,
    } = coordinator.accept(&start(3)).unwrap()
    else {
        panic!("expected a key rotation action");
    };
    assert_eq!(from, previous);
    assert_eq!(coordinator.active(), Some(previous));
    coordinator.complete_rotation(from, next).unwrap();
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
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiation());
    activate(&mut coordinator, 4);
    let active = coordinator.active().unwrap();

    assert_eq!(
        coordinator.accept(&stop(3)).unwrap(),
        ControlStreamAction::Noop
    );
    assert_eq!(coordinator.active(), Some(active));
    assert_eq!(
        coordinator.accept(&stop(4)).unwrap(),
        ControlStreamAction::Stop(active)
    );
    coordinator
        .state_mut()
        .apply(SessionEvent::StopStream)
        .unwrap();
    coordinator.complete_stop(active).unwrap();
    assert_eq!(coordinator.active(), None);
}

#[test]
fn invalid_control_frame_cannot_prepare_media_work() {
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiation());
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
    let negotiated = negotiate_hello(
        &Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "Lab sender".to_owned(),
        },
        &[Capability::FrameDuration20Ms],
    )
    .unwrap();
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiated);
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
        Ok(ControlStreamAction::Start(_))
    ));
}

#[test]
fn accepts_selected_10_ms_start_and_rotation() {
    let mut coordinator = ControlStreamCoordinator::new(trusted_session(), negotiation());
    let ControlStreamAction::Start(initial) =
        coordinator.accept(&start_with_duration(2, 10)).unwrap()
    else {
        panic!("expected 10 ms start action");
    };
    coordinator
        .state_mut()
        .apply(SessionEvent::StartStream)
        .unwrap();
    coordinator.complete_start(initial).unwrap();

    let ControlStreamAction::Rotate { previous, next } =
        coordinator.accept(&start_with_duration(3, 10)).unwrap()
    else {
        panic!("expected 10 ms rotation action");
    };
    coordinator.complete_rotation(previous, next).unwrap();
    assert_eq!(coordinator.active(), Some(next.identity));
}
