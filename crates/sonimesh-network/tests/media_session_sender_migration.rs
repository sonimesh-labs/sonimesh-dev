//! Loopback proof that a media session sender migrates its routing destination
//! without rotating media keys.
//!
//! `MediaSessionSender::set_peer` is documented as routing metadata only: a
//! control-session address migration updates the destination while the same
//! session, stream, direction, key epoch, and per-epoch sequence continue. A
//! fresh receiver with the same authenticated context must accept the next
//! packets without any key rotation or sequence reset.

use std::thread;

use sonimesh_network::{
    MediaSessionSendOutcome, MediaSessionSender, ProtectedMediaIngress, UdpMediaReceiver,
};
use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaDirection, MediaHeader, MediaKeyContext, SessionSecret,
};

fn context() -> MediaKeyContext {
    MediaKeyContext::new(7, 9, 1, MediaDirection::AndroidToDesktop).unwrap()
}

fn secret() -> SessionSecret {
    SessionSecret::new([0xA5; 32])
}

fn sender() -> MediaSessionSender {
    MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(),
        context(),
        "127.0.0.1:9".parse().unwrap(),
    )
    .unwrap()
}

fn receiver() -> (UdpMediaReceiver, ProtectedMediaIngress) {
    let receiver = UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let ingress = ProtectedMediaIngress::new(&secret(), context()).unwrap();
    (receiver, ingress)
}

/// Polls until one datagram authenticates, returning its header and plaintext.
fn receive_next(
    receiver: &mut UdpMediaReceiver,
    ingress: &mut ProtectedMediaIngress,
) -> (MediaHeader, Vec<u8>) {
    for _ in 0..1_000 {
        if let Some(datagram) = receiver.poll().unwrap() {
            let mut plaintext = [0_u8; MAX_MEDIA_PLAINTEXT_LEN];
            let frame = ingress.receive(datagram.payload, &mut plaintext).unwrap();
            return (frame.header, frame.opus_payload.to_vec());
        }
        thread::yield_now();
    }
    panic!("loopback datagram was not delivered");
}

/// Sends one frame and asserts the exact per-epoch sequence was consumed.
fn send_frame(sender: &mut MediaSessionSender, frame: u32) {
    let outcome = sender
        .send_frame(0, u64::from(frame) * 960, &[u8::try_from(frame).unwrap()])
        .unwrap();
    assert_eq!(outcome, MediaSessionSendOutcome::Sent { sequence: frame });
}

#[test]
fn set_peer_migrates_routing_without_rotating_keys_or_resetting_sequence() {
    let mut sender = sender();
    let (mut receiver_a, mut ingress_a) = receiver();
    sender.set_peer(receiver_a.local_addr().unwrap());

    // Send the first frames to receiver A; the sequence starts at zero.
    for frame in 0..3 {
        send_frame(&mut sender, frame);
    }
    for frame in 0..3 {
        let (header, plaintext) = receive_next(&mut receiver_a, &mut ingress_a);
        assert_eq!(header.sequence, frame);
        assert_eq!(
            plaintext,
            [u8::try_from(frame).expect("test frame fits one byte")]
        );
    }

    // Receiver B is a fresh endpoint with the same authenticated context.
    let (mut receiver_b, mut ingress_b) = receiver();

    // Migrate routing only; the context and per-epoch sequence must survive.
    sender.set_peer(receiver_b.local_addr().unwrap());
    assert_eq!(sender.context(), context());

    for frame in 3..6 {
        send_frame(&mut sender, frame);
    }
    for frame in 3..6 {
        let (header, plaintext) = receive_next(&mut receiver_b, &mut ingress_b);
        assert_eq!(header.sequence, frame);
        assert_eq!(header.session_id, context().session_id);
        assert_eq!(header.stream_id, context().stream_id);
        assert_eq!(header.key_epoch, context().key_epoch);
        assert_eq!(
            plaintext,
            [u8::try_from(frame).expect("test frame fits one byte")]
        );
    }

    // The old destination no longer receives packets from this sender.
    for _ in 0..100 {
        assert_eq!(receiver_a.poll().unwrap(), None);
        thread::yield_now();
    }
}
