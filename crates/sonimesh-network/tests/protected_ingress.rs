//! End-to-end authentication and replay ordering using the protocol AEAD adapter.

use sonimesh_network::{
    IngressError, MediaIngress, ProtectedIngressError, ProtectedMediaIngress,
    ProtocolAuthenticator, StreamIdentity,
};
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

fn context_at_epoch(epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        0xA1B2_C3D4_E5F6_1020,
        9,
        epoch,
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

#[test]
fn rotation_replaces_aead_and_replay_state_without_accepting_old_packets() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context_at_epoch(2);
    let mut old_sealer = MediaSealer::new(&secret, initial).unwrap();
    let mut ingress = ProtectedMediaIngress::new(&secret, initial).unwrap();
    let old_packet = old_sealer.seal(0, 0, b"old epoch").unwrap();
    let mut plaintext = [0_u8; 64];

    assert_eq!(
        ingress
            .receive(&old_packet.datagram, &mut plaintext)
            .unwrap()
            .opus_payload,
        b"old epoch"
    );
    let next = context_at_epoch(3);
    ingress.rotate(&secret, next).unwrap();
    assert_eq!(ingress.context(), next);
    assert_eq!(ingress.identity().key_epoch, 3);

    plaintext.fill(0xFF);
    assert_eq!(
        ingress.receive(&old_packet.datagram, &mut plaintext),
        Err(IngressError::UnknownStream)
    );
    assert!(plaintext.iter().all(|byte| *byte == 0));

    let mut new_sealer = MediaSealer::new(&secret, next).unwrap();
    let next_packet = new_sealer.seal(0, 0, b"new epoch").unwrap();
    assert_eq!(
        ingress
            .receive(&next_packet.datagram, &mut plaintext)
            .unwrap()
            .opus_payload,
        b"new epoch"
    );
}

#[test]
fn rotation_rejects_non_increasing_or_changed_context_without_mutation() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context_at_epoch(2);
    let mut ingress = ProtectedMediaIngress::new(&secret, initial).unwrap();

    assert_eq!(
        ingress.rotate(&secret, initial),
        Err(ProtectedIngressError::NonIncreasingEpoch)
    );
    let wrong_stream = MediaKeyContext::new(
        initial.session_id,
        initial.stream_id + 1,
        3,
        initial.direction,
    )
    .unwrap();
    assert_eq!(
        ingress.rotate(&secret, wrong_stream),
        Err(ProtectedIngressError::RotationContextMismatch)
    );
    assert_eq!(ingress.context(), initial);
}
