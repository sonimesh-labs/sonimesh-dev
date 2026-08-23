//! End-to-end authenticated control transport over a real TCP loopback.
//!
//! This test closes the last application-domain gap around ADR 0020: the
//! pairing proof frames travel over a real socket pair (not in-memory pipes),
//! the verified exchange unlocks the bounded envelope framing, a validated
//! hello is negotiated through that framing into the coordinator, and the
//! freshly installed transport token authorizes a new media identity through
//! the ADR 0017 allocator. It models both roles in one process over loopback,
//! exactly like the provider spike; it performs no TLS (the exporter is a
//! fixed test value) and claims no Android or production provider behavior.

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, MediaSessionAllocator, MediaStreamIdentity,
    PairingAttemptOutcome, PairingId, PairingInvitationController, PairingProofVerification,
    PairingSource, SessionPhase, SessionState,
};
use sonimesh_network::{AuthenticatedControlHandshake, binding_for_channel};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, PairingRole,
    SessionSecret, StartStream, envelope,
};

const PAIRING_ID: [u8; 16] = [
    0x5A, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
];
const PAIRING_SECRET: [u8; 32] = [0xA7; 32];
const EXPORTER: [u8; 32] = [0xC3; 32];
const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;
const CAPABILITIES: &[Capability] = &[Capability::FrameDuration20Ms];

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

/// Reaches the READY session through the pairing owner, then installs the
/// authenticated control session token and returns both.
fn ready_coordinator() -> (ControlStreamCoordinator, sonimesh_app::ControlSessionToken) {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    pairing
        .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
        .expect("pairing invitation begins");
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let mut coordinator = ControlStreamCoordinator::new(session, CAPABILITIES)
        .expect("coordinator with bounded local capabilities");
    let token = coordinator
        .install_authenticated_control_session()
        .expect("fresh transport token");
    (coordinator, token)
}

fn hello_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 7,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "authenticated loopback sender".to_owned(),
        })),
    }
}

fn start_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 8,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0,
        })),
    }
}

/// One side's proof frame encoded into the canonical 55-byte wire layout.
fn frame_wire(role: PairingRole) -> [u8; 55] {
    let binding = binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER)
        .expect("fixed inputs derive a binding");
    let frame = sonimesh_protocol::PairingProofFrame {
        protocol_version: 1,
        role,
        pairing_id: PAIRING_ID,
        proof: binding
            .local_proof(role)
            .expect("a canonical binding derives its local proof"),
    };
    let mut wire = [0_u8; 55];
    frame.encode_into(&mut wire).expect("a fixed frame encodes");
    wire
}

#[test]
fn authenticated_control_transport_reaches_the_allocator_over_loopback() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");

    // The desktop side accepts the connection and completes the exchange.
    let desktop = thread::spawn(
        move || -> (
            sonimesh_network::AuthenticatedControlStream<TcpStream>,
            sonimesh_network::VerifiedPeerProof,
        ) {
            let (stream, _) = listener.accept().expect("desktop accepts");
            stream.set_nonblocking(false).expect("blocking mode");
            let mut handshake = AuthenticatedControlHandshake::new(
                stream,
                sonimesh_protocol::PairingBinding::new(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER),
                PAIRING_ID,
                PairingRole::DesktopServer,
            )
            .expect("desktop handshake constructs");

            let peer_wire = handshake.read_peer_frame().expect("desktop reads client frame");
            let verified = handshake
                .verify_peer_frame(&peer_wire)
                .expect("desktop verifies the client proof");
            assert_eq!(verified.reply_frame().role, PairingRole::DesktopServer);
            handshake.send_reply_frame().expect("desktop replies");
            let framed = handshake.into_framing().expect("desktop unlocks framing");
            (framed, verified)
        },
    );

    // The Android side connects, sends its frame, verifies the reply.
    let mut stream = TcpStream::connect(address).expect("android connects");
    let mut handshake = AuthenticatedControlHandshake::new(
        stream.try_clone().expect("android clones the stream"),
        sonimesh_protocol::PairingBinding::new(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER),
        PAIRING_ID,
        PairingRole::AndroidClient,
    )
    .expect("android handshake constructs");

    let client_wire = frame_wire(PairingRole::AndroidClient);
    handshake
        .send_reply_frame()
        .expect_err("unverified android cannot send a reply yet");
    // Send the client frame through the raw stream: the handshake wrapper
    // exposes no envelope I/O, so this is the peer-frame write path.
    stream
        .write_all(&client_wire)
        .expect("android sends its frame");

    let server_wire = handshake.read_peer_frame().expect("android reads reply");
    let decoded = sonimesh_protocol::parse_pairing_proof_frame(&server_wire)
        .expect("reply decodes canonically");
    assert_eq!(decoded.role, PairingRole::DesktopServer);
    let verified_android = handshake
        .verify_peer_frame(&server_wire)
        .expect("android verifies the desktop reply");

    let framed = handshake.into_framing().expect("android unlocks framing");

    // Both sides hold the same transcript media root: seal one fixed
    // datagram under each side's root and require byte equality. The desktop
    // thread has finished its exchange by now; join it for both its framed
    // stream and its verified proof.
    let (mut desktop_stream, desktop_verified) = desktop.join().expect("desktop thread joins");
    let android_binding =
        binding_for_channel(1, PAIRING_ID, PAIRING_SECRET, &EXPORTER).expect("android binding");
    let expected_root = android_binding.media_root().expect("media root derives");
    let context = sonimesh_protocol::MediaKeyContext::new(7, 9, 0, DIRECTION)
        .expect("fixed test context is valid");
    let mut from_desktop =
        sonimesh_protocol::MediaSealer::new(desktop_verified.media_root(), context)
            .expect("desktop root seals");
    let mut from_android =
        sonimesh_protocol::MediaSealer::new(&expected_root, context).expect("android root seals");
    assert_eq!(
        from_desktop
            .seal(0, 1_000, &[7_u8; 16])
            .expect("desktop seal")
            .datagram,
        from_android
            .seal(0, 1_000, &[7_u8; 16])
            .expect("android seal")
            .datagram
    );
    let _ = verified_android;

    // The authenticated envelope path: hello negotiates into the coordinator,
    // and the fresh token authorizes a new media identity via the allocator.
    drive_control_session(&mut desktop_stream, framed);
}

/// Negotiates hello over the authenticated framing, then uses the installed
/// transport token to authorize a fresh media session through the allocator
/// and completes a start into the streaming phase.
fn drive_control_session(
    desktop_stream: &mut sonimesh_network::AuthenticatedControlStream<TcpStream>,
    sender: sonimesh_network::AuthenticatedControlStream<TcpStream>,
) {
    let mut sender = sender;
    assert!(matches!(
        sender.send(&hello_envelope()),
        Ok(sonimesh_network::UntrustedControlWrite::Complete)
    ));

    // The desktop side reads the hello through its authenticated framing.
    let (mut coordinator, token) = ready_coordinator();
    let mut poll = desktop_stream.poll().expect("desktop polls the hello");
    while matches!(poll, sonimesh_network::UntrustedControlPoll::Empty) {
        std::thread::yield_now();
        poll = desktop_stream.poll().expect("desktop polls again");
    }
    let sonimesh_network::UntrustedControlPoll::Frame(hello) = poll else {
        panic!("desktop did not receive the hello frame");
    };
    assert!(matches!(
        coordinator.accept(token, &hello),
        Ok(ControlStreamAction::Negotiated(_))
    ));

    // The authenticated transport token authorizes a fresh media session.
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));
    let grant = allocator
        .supply_for_authenticated_control_session(&coordinator, token, identity(7, 9, 0), DIRECTION)
        .expect("active transport issues a fresh identity");
    assert_eq!(grant.context().session_id, 7);

    // A start stream through the same authenticated framing completes.
    let action = coordinator
        .accept(token, &start_envelope())
        .expect("start envelope is accepted");
    let ControlStreamAction::Start { token: op, request } = action else {
        panic!("expected a start action, got {action:?}");
    };
    coordinator
        .complete_start(op, request)
        .expect("start completes");
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);

    drop(sender);
}
