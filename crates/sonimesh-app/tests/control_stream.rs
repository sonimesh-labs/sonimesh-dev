//! Low-rate control lifecycle tests independent of output and network adapters.

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, ControlStreamError, SessionEvent, SessionState,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, ControlFrameError, Envelope, MediaDirection, StartStream, StopStream,
    envelope,
};

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
    let mut coordinator = ControlStreamCoordinator::new(SessionState::Ready);
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
    let mut coordinator = ControlStreamCoordinator::new(SessionState::Ready);
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
    let mut coordinator = ControlStreamCoordinator::new(SessionState::Ready);
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
    let mut coordinator = ControlStreamCoordinator::new(SessionState::Ready);
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
