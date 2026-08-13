//! Wire-level control-plane integration tests.
//!
//! These prove the full receiver path that no unit test exercises alone: a
//! length-prefixed control frame travels over a real byte stream, is bounded
//! and semantically validated by `UntrustedControlStream` framing, and then
//! drives the authenticated `ControlStreamCoordinator` lifecycle. Transport
//! authentication is deliberately absent between the two crates; the framing
//! boundary must reject malformed input before the coordinator can see it.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, PairingAttemptOutcome,
    PairingId, PairingInvitationController, PairingProofVerification, PairingSource, SessionPhase,
    SessionState,
};
use sonimesh_network::{UntrustedControlError, UntrustedControlPoll, UntrustedControlStream};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello, StartStream,
    StopStream, StreamState, StreamStatus, encode_control_frame, envelope,
};

/// Builds a paired, authenticated control coordinator in the ready phase.
fn authenticated_coordinator() -> (ControlStreamCoordinator, ControlSessionToken) {
    let mut coordinator =
        ControlStreamCoordinator::new(trusted_session(), &LOCAL_CAPABILITIES).unwrap();
    let token = coordinator.install_authenticated_control_session().unwrap();
    (coordinator, token)
}

/// Reaches the ready session through the pairing owner, exactly like the
/// control-domain lifecycle would after an accepted transcript-bound proof.
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

/// Local receiver capability policy used by every coordinator in these tests.
const LOCAL_CAPABILITIES: [Capability; 2] =
    [Capability::FrameDuration20Ms, Capability::ManualAddress];

fn hello_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 7,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![
                Capability::FrameDuration20Ms as i32,
                Capability::ManualAddress as i32,
                9_999,
            ],
            display_name: "wire sender".to_owned(),
        })),
    }
}

fn start_envelope(epoch: u32) -> Envelope {
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

fn stop_envelope(epoch: u32) -> Envelope {
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

fn status_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 10,
        payload: Some(envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: 7,
            stream_id: 9,
            state: StreamState::Streaming as i32,
            detail: "receiving".to_owned(),
        })),
    }
}

/// One loopback peer pair: `sender` is a blocking TCP stream, `reader` is the
/// non-blocking framed control reader owning the accepted connection.
fn connected_pair() -> (TcpStream, UntrustedControlStream<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let sender = TcpStream::connect(address).unwrap();
    let (receiver, _) = listener.accept().unwrap();
    (
        sender,
        UntrustedControlStream::from_stream(receiver).unwrap(),
    )
}

/// Encodes and writes one validated control frame to the wire.
fn send_frame(sender: &mut TcpStream, envelope: &Envelope) {
    let frame = encode_control_frame(envelope).unwrap();
    sender.write_all(&frame).unwrap();
}

/// Polls the framed reader until exactly one complete control frame arrives.
fn receive_frame(reader: &mut UntrustedControlStream<TcpStream>) -> Envelope {
    for _ in 0..2_000 {
        match reader.poll().unwrap() {
            UntrustedControlPoll::Empty => thread::yield_now(),
            UntrustedControlPoll::Frame(envelope) => return envelope,
            UntrustedControlPoll::Closed => panic!("control stream closed before a frame arrived"),
        }
    }
    panic!("no complete control frame arrived on the wire");
}

#[test]
fn hello_frame_negotiates_canonical_capabilities_over_the_wire() {
    let (mut sender, mut reader) = connected_pair();
    let (mut coordinator, token) = authenticated_coordinator();

    send_frame(&mut sender, &hello_envelope());
    let envelope = receive_frame(&mut reader);

    let action = coordinator.accept(token, &envelope).unwrap();
    let ControlStreamAction::Negotiated(negotiated) = action else {
        panic!("expected a negotiated action, got {action:?}");
    };
    assert_eq!(negotiated.version(), CONTROL_PROTOCOL_VERSION);
    assert_eq!(
        negotiated.capabilities(),
        [Capability::FrameDuration20Ms, Capability::ManualAddress]
    );
    assert!(!negotiated.supports(Capability::NetworkDiagnostics));
}

#[test]
fn start_and_stop_commands_flow_over_the_wire_and_commit_lifecycle() {
    let (mut sender, mut reader) = connected_pair();
    let (mut coordinator, token) = authenticated_coordinator();

    send_frame(&mut sender, &hello_envelope());
    let envelope = receive_frame(&mut reader);
    assert!(matches!(
        coordinator.accept(token, &envelope).unwrap(),
        ControlStreamAction::Negotiated(_)
    ));

    send_frame(&mut sender, &start_envelope(0x2345));
    let envelope = receive_frame(&mut reader);
    let ControlStreamAction::Start {
        token: start_token,
        request,
    } = coordinator.accept(token, &envelope).unwrap()
    else {
        panic!("expected a start action after the wire hello");
    };
    assert_eq!(request.identity.session_id, 7);
    assert_eq!(request.identity.stream_id, 9);
    assert_eq!(request.identity.key_epoch, 0x2345);
    assert_eq!(request.sample_rate_hz, 48_000);
    assert_eq!(request.channels, 2);
    assert_eq!(request.frame_duration_ms, 20);

    coordinator.complete_start(start_token, request).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);
    assert_eq!(coordinator.active(), Some(request.identity));

    send_frame(&mut sender, &stop_envelope(0x2345));
    let envelope = receive_frame(&mut reader);
    let ControlStreamAction::Stop {
        token: stop_token,
        identity,
    } = coordinator.accept(token, &envelope).unwrap()
    else {
        panic!("expected a stop action after the wire start");
    };
    assert_eq!(identity, request.identity);

    coordinator.complete_stop(stop_token, identity).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Ready);
    assert_eq!(coordinator.active(), None);
}

#[test]
fn status_frame_is_ignored_but_still_arrives_over_the_wire() {
    let (mut sender, mut reader) = connected_pair();
    let (mut coordinator, token) = authenticated_coordinator();

    send_frame(&mut sender, &status_envelope());
    let envelope = receive_frame(&mut reader);

    assert_eq!(
        coordinator.accept(token, &envelope).unwrap(),
        ControlStreamAction::Ignored
    );
}

#[test]
fn malformed_wire_frame_terminates_the_stream_before_the_coordinator() {
    let (mut sender, mut reader) = connected_pair();
    let (coordinator, _) = authenticated_coordinator();

    // Mutate the hello's protocol-version varint from 1 to 2 after encoding so
    // the stream sees bytes that its own bounded encoder would never produce.
    let mut frame = encode_control_frame(&hello_envelope()).unwrap();
    let version_offset = frame
        .windows(2)
        .position(|window| window == [0x08, 0x01])
        .expect("encoded hello contains the protocol-version field");
    assert_eq!(frame[version_offset + 1], 0x01);
    frame[version_offset + 1] = 0x02;
    sender.write_all(&frame).unwrap();

    let error = loop {
        match reader.poll() {
            Err(error) => break error,
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            Ok(UntrustedControlPoll::Frame(_)) => panic!("malformed frame reached the coordinator"),
            Ok(UntrustedControlPoll::Closed) => {
                panic!("stream closed before rejecting the malformed frame")
            }
        }
    };
    assert!(matches!(
        error,
        UntrustedControlError::Protocol(ControlFrameError::UnsupportedVersion(2))
    ));
    assert!(matches!(
        reader.poll().unwrap(),
        UntrustedControlPoll::Closed
    ));

    // The coordinator never saw the invalid envelope, so its state is untouched.
    assert_eq!(coordinator.state().phase(), SessionPhase::Ready);
    assert_eq!(coordinator.negotiation(), None);
}

#[test]
fn expired_control_session_rejects_a_late_wire_frame() {
    let (mut sender, mut reader) = connected_pair();
    let (mut coordinator, token) = authenticated_coordinator();

    send_frame(&mut sender, &hello_envelope());
    let envelope = receive_frame(&mut reader);
    assert!(matches!(
        coordinator.accept(token, &envelope).unwrap(),
        ControlStreamAction::Negotiated(_)
    ));

    coordinator.expire_control_session();
    assert_eq!(coordinator.state(), SessionState::IDLE);

    // A start command can still travel over the wire and be framed, but the
    // retired token must keep it away from the idle coordinator.
    send_frame(&mut sender, &start_envelope(0x2345));
    let envelope = receive_frame(&mut reader);
    assert!(matches!(
        coordinator.accept(token, &envelope),
        Err(sonimesh_app::ControlStreamError::StaleControlSession)
    ));
    assert_eq!(coordinator.state(), SessionState::IDLE);
}

#[test]
fn key_epoch_rotation_flows_over_the_wire_and_keeps_the_stream_active() {
    let (mut sender, mut reader) = connected_pair();
    let (mut coordinator, token) = authenticated_coordinator();

    send_frame(&mut sender, &hello_envelope());
    let envelope = receive_frame(&mut reader);
    assert!(matches!(
        coordinator.accept(token, &envelope).unwrap(),
        ControlStreamAction::Negotiated(_)
    ));

    send_frame(&mut sender, &start_envelope(0x2345));
    let envelope = receive_frame(&mut reader);
    let ControlStreamAction::Start {
        token: start_token,
        request,
    } = coordinator.accept(token, &envelope).unwrap()
    else {
        panic!("expected a start action after the wire hello");
    };
    coordinator.complete_start(start_token, request).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);
    assert_eq!(coordinator.active(), Some(request.identity));

    // A start for a strictly newer epoch over the same connection rotates the
    // stream in place instead of stopping and restarting it.
    send_frame(&mut sender, &start_envelope(0x2346));
    let envelope = receive_frame(&mut reader);
    let ControlStreamAction::Rotate {
        token: rotate_token,
        previous,
        next,
    } = coordinator.accept(token, &envelope).unwrap()
    else {
        panic!("expected a rotation action for the newer epoch");
    };
    assert_eq!(previous, request.identity);
    assert_eq!(next.identity.session_id, request.identity.session_id);
    assert_eq!(next.identity.stream_id, request.identity.stream_id);
    assert_eq!(next.identity.key_epoch, 0x2346);
    coordinator
        .complete_rotation(rotate_token, previous, next)
        .unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);
    assert_eq!(coordinator.active(), Some(next.identity));

    // A stale start for the retired epoch is rejected and cannot disturb the
    // active rotated stream.
    send_frame(&mut sender, &start_envelope(0x2345));
    let envelope = receive_frame(&mut reader);
    assert!(matches!(
        coordinator.accept(token, &envelope),
        Err(sonimesh_app::ControlStreamError::StaleEpoch {
            active: 0x2346,
            requested: 0x2345,
        })
    ));
    assert_eq!(coordinator.active(), Some(next.identity));

    // The active rotated epoch stops cleanly over the wire.
    send_frame(&mut sender, &stop_envelope(0x2346));
    let envelope = receive_frame(&mut reader);
    let ControlStreamAction::Stop {
        token: stop_token,
        identity,
    } = coordinator.accept(token, &envelope).unwrap()
    else {
        panic!("expected a stop action for the rotated epoch");
    };
    assert_eq!(identity, next.identity);
    coordinator.complete_stop(stop_token, identity).unwrap();
    assert_eq!(coordinator.state().phase(), SessionPhase::Ready);
    assert_eq!(coordinator.active(), None);
}
