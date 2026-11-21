//! Structured control-plane error source-chain tests.

use std::error::Error;

use sonimesh_app::{
    ControlListenError, ControlStreamError, PairingInvitationError, PairingRecordError,
    SessionEvent, SessionState, SessionTransitionError,
};
use sonimesh_protocol::{ControlFrameError, ControlNegotiationError};

fn source<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    error
        .source()
        .expect("composite control error must retain its cause")
}

#[test]
fn control_negotiation_retains_the_peer_protocol_cause() {
    let error = ControlStreamError::Negotiation(ControlNegotiationError::PeerHello(
        ControlFrameError::TruncatedLength,
    ));

    let negotiation = source(&error);
    assert!(
        negotiation
            .downcast_ref::<ControlNegotiationError>()
            .is_some()
    );
    let protocol = source(negotiation);
    assert!(protocol.downcast_ref::<ControlFrameError>().is_some());
    assert!(protocol.source().is_none());
}

#[test]
fn control_transition_retains_the_session_cause() {
    let transition = SessionTransitionError {
        state: SessionState::IDLE,
        event: SessionEvent::StartStream,
    };
    let error = ControlStreamError::Transition(transition);

    assert!(
        source(&error)
            .downcast_ref::<SessionTransitionError>()
            .is_some()
    );
    assert!(Error::source(&ControlStreamError::MissingNegotiation).is_none());
}

#[test]
fn pairing_errors_retain_record_and_transition_causes() {
    let record = PairingInvitationError::Record(PairingRecordError::TimeRegressed);
    assert!(
        source(&record)
            .downcast_ref::<PairingRecordError>()
            .is_some()
    );

    let transition = PairingInvitationError::Transition(SessionTransitionError {
        state: SessionState::IDLE,
        event: SessionEvent::StartStream,
    });
    assert!(
        source(&transition)
            .downcast_ref::<SessionTransitionError>()
            .is_some()
    );
    assert!(Error::source(&PairingInvitationError::MissingInvitation).is_none());
}

#[test]
fn pairing_listener_io_errors_retain_the_socket_cause() {
    let io_error = std::io::Error::new(std::io::ErrorKind::TimedOut, "test timeout");
    let error = ControlListenError::PeerFrameIo(io_error);

    let cause = source(&error);
    assert_eq!(
        cause.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::TimedOut
    );
}
