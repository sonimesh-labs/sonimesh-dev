//! End-to-end pairing handshake composition tests over real components.
//!
//! These tests drive the full desktop-side first-pair flow with no mocks:
//! the invitation is rendered through the bounded QR encoder, scanned back
//! through the bounded decoder, verified against the real
//! [`PairingBinding`] HKDF transcript, admitted by the real invitation
//! owner, and the resulting media root drives the real
//! [`MediaSealer`]/[`MediaOpener`] AEAD and the [`MediaSessionAllocator`]
//! freshness record. No TLS, camera, or network transport is exercised;
//! the exporter bytes model what a pinned TLS 1.3 connection would export.

use sonimesh_app::{
    MediaSessionAllocator, PairingHandshakeOutcome, PairingId, PairingInvitationController,
    PairingQrInvitation, ScannedProofAttempt, begin_from_rendered_payload, verify_scanned_proof,
};
use sonimesh_protocol::{
    MAX_QR_PAYLOAD_LEN, MediaDirection, MediaKeyContext, PAIRING_TLS_EXPORTER_LEN,
};

const HOST: &str = "desktop-01.lab.local";
const PORT: u16 = 47_610;
/// Media session ID used to prove the handshake root drives the AEAD layer.
const SESSION_ID: u64 = 0x1122_3344_5566_7788;

/// Deterministic non-secret lab secret: bytes 3, 10, 17, ... mod 256.
const fn lab_pairing_secret() -> [u8; 32] {
    let mut secret = [0_u8; 32];
    let mut byte = 3_u8;
    let mut index = 0;
    while index < secret.len() {
        secret[index] = byte;
        byte = byte.wrapping_add(7);
        index += 1;
    }
    secret
}

struct RenderedInvitation {
    pairing: PairingInvitationController<4>,
    id: PairingId,
    secret: [u8; 32],
    payload: Vec<u8>,
}

fn render(now_ms: u64) -> (RenderedInvitation, sonimesh_app::SessionState) {
    let mut session = sonimesh_app::SessionState::IDLE;
    let mut pairing = PairingInvitationController::<4>::new();
    let id = PairingId::new([
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x0F,
        0x01,
    ]);
    let secret = lab_pairing_secret();
    let mut payload_out = vec![0_u8; MAX_QR_PAYLOAD_LEN];
    let written = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        now_ms,
        60_000,
        2,
        PairingQrInvitation {
            id,
            pairing_secret: secret,
            control_host: HOST,
            control_port: PORT,
            desktop_tls_spki: &[0x30; 91],
            expires_at_ms: now_ms + 60_000,
        },
        &mut payload_out,
    )
    .expect("a canonical lab invitation renders");
    payload_out.truncate(written);
    (
        RenderedInvitation {
            pairing,
            id,
            secret,
            payload: payload_out,
        },
        session,
    )
}

#[test]
fn rendered_scan_verify_flow_reaches_ready_and_drives_the_allocator() {
    let (mut rendered, mut session) = render(1_000);
    let exporter = vec![0xC7_u8; PAIRING_TLS_EXPORTER_LEN];

    // Android derives its client proof from the scanned public fields.
    let peer_binding = sonimesh_protocol::PairingBinding::new(
        1,
        rendered.id.as_bytes(),
        rendered.secret,
        &exporter,
    )
    .expect("the scan carries valid binding inputs");
    let client_proof = peer_binding
        .local_proof(sonimesh_protocol::PairingRole::AndroidClient)
        .expect("client proof derivation");

    let outcome = verify_scanned_proof(
        &mut rendered.pairing,
        &mut session,
        1_500,
        sonimesh_app::PairingSource::new([0xA1; 16]),
        &exporter,
        ScannedProofAttempt {
            scanned_payload: &rendered.payload,
            expected_pairing_id: rendered.id,
            pairing_secret: rendered.secret,
            peer_proof: &client_proof,
        },
    )
    .expect("the correct proof completes the handshake");

    let PairingHandshakeOutcome::Ready { media_root } = outcome else {
        panic!("the trusted outcome must carry the media root");
    };
    assert_eq!(session.phase(), sonimesh_app::SessionPhase::Ready);

    // The root seals and opens media exactly like an ADR 0005 transcript root.
    let context = MediaKeyContext {
        session_id: SESSION_ID,
        stream_id: 1,
        key_epoch: 0,
        direction: MediaDirection::AndroidToDesktop,
    };
    let mut sender = sonimesh_protocol::MediaSealer::new(&media_root, context)
        .expect("the handshake root derives a sender key");
    let sealed = sender.seal(0, 9_000, &[0x42_u8; 64]).expect("seal");
    let receiver = sonimesh_protocol::MediaOpener::new(&media_root, context)
        .expect("the handshake root derives a receiver key");
    let mut plaintext = [0_u8; 64];
    let parsed = sonimesh_protocol::parse_media_packet(&sealed.datagram)
        .expect("the sealed datagram parses");
    let opened = receiver
        .open(parsed, &mut plaintext)
        .expect("the matching opener authenticates the packet");
    assert_eq!(opened, 64);
    assert_eq!(plaintext, [0x42_u8; 64]);

    // The same root backs the ADR 0017 allocator and issues one fresh grant.
    let mut allocator = MediaSessionAllocator::new(media_root);
    let identity = sonimesh_app::MediaStreamIdentity {
        session_id: SESSION_ID,
        stream_id: 1,
        key_epoch: 0,
    };
    let grant = allocator
        .supply(identity, MediaDirection::AndroidToDesktop)
        .expect("the fresh pairing root issues the first identity");
    assert_eq!(grant.context().session_id, SESSION_ID);

    // Replaying the same identity inside this pairing lifetime fails closed.
    assert!(matches!(
        allocator.supply(identity, MediaDirection::AndroidToDesktop),
        Err(sonimesh_app::MediaSessionAllocatorError::IdentityReused)
    ));
}

#[test]
fn the_same_transcript_on_both_sides_yields_matching_proofs_and_roots() {
    // Simulate both peers deriving from one scanned invitation plus one
    // exporter: the desktop must accept exactly the Android-role proof, and
    // both derivations of the media root must seal identical datagrams.
    let (mut rendered, mut session) = render(2_000);
    let exporter = vec![0x3D_u8; PAIRING_TLS_EXPORTER_LEN];
    let shared = sonimesh_protocol::PairingBinding::new(
        1,
        rendered.id.as_bytes(),
        rendered.secret,
        &exporter,
    )
    .unwrap();
    let client_proof = shared
        .local_proof(sonimesh_protocol::PairingRole::AndroidClient)
        .unwrap();

    let outcome = verify_scanned_proof(
        &mut rendered.pairing,
        &mut session,
        2_100,
        sonimesh_app::PairingSource::new([0xB2; 16]),
        &exporter,
        ScannedProofAttempt {
            scanned_payload: &rendered.payload,
            expected_pairing_id: rendered.id,
            pairing_secret: rendered.secret,
            peer_proof: &client_proof,
        },
    )
    .expect("handshake completes");
    let PairingHandshakeOutcome::Ready { media_root } = outcome else {
        panic!("expected the ready outcome");
    };

    let expected_media_root = shared.media_root().unwrap();
    let context = MediaKeyContext {
        session_id: 1,
        stream_id: 1,
        key_epoch: 0,
        direction: MediaDirection::AndroidToDesktop,
    };
    let mut from_handshake = sonimesh_protocol::MediaSealer::new(&media_root, context).unwrap();
    let mut from_shared =
        sonimesh_protocol::MediaSealer::new(&expected_media_root, context).unwrap();
    let left = from_handshake.seal(0, 5, &[1_u8; 32]).unwrap();
    let right = from_shared.seal(0, 5, &[1_u8; 32]).unwrap();
    assert_eq!(left.datagram, right.datagram);
}
