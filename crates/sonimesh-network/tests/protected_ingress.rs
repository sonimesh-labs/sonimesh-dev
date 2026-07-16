//! End-to-end authentication and replay ordering using the protocol AEAD adapter.

use sonimesh_network::{IngressError, MediaIngress, ProtocolAuthenticator, StreamIdentity};
use sonimesh_protocol::{
    MediaDirection, MediaKeyContext, MediaOpener, MediaSealer, ReplayDecision, SessionSecret,
};

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0xA1B2_C3D4_E5F6_1020,
        9,
        2,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

#[test]
fn authenticates_before_advancing_replay_window() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let mut ingress = MediaIngress::new(
        StreamIdentity {
            session_id: context.session_id,
            stream_id: context.stream_id,
            key_epoch: context.key_epoch,
        },
        ProtocolAuthenticator::new(opener),
    );
    let protected = sealer.seal(0, 0, b"authenticated opus bytes").unwrap();
    let mut plaintext = [0_u8; 64];

    let frame = ingress
        .receive(&protected.datagram, &mut plaintext)
        .unwrap();
    assert_eq!(frame.opus_payload, b"authenticated opus bytes");

    let duplicate = ingress
        .receive(&protected.datagram, &mut plaintext)
        .unwrap_err();
    assert_eq!(duplicate, IngressError::Replay(ReplayDecision::Duplicate));
    assert_eq!(plaintext[0], 0);
}

#[test]
fn failed_aead_does_not_mark_sequence_received() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let opener = MediaOpener::new(&secret, context).unwrap();
    let mut ingress = MediaIngress::new(
        StreamIdentity {
            session_id: context.session_id,
            stream_id: context.stream_id,
            key_epoch: context.key_epoch,
        },
        ProtocolAuthenticator::new(opener),
    );
    let protected = sealer.seal(0, 0, b"frame").unwrap();
    let mut tampered = protected.datagram.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0x80;
    let mut plaintext = [0xFF; 16];

    assert!(matches!(
        ingress.receive(&tampered, &mut plaintext),
        Err(IngressError::Authentication(_))
    ));
    assert!(plaintext.iter().all(|byte| *byte == 0));
    assert!(ingress.receive(&protected.datagram, &mut plaintext).is_ok());
}
