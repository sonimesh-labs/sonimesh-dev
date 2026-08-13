//! Non-blocking bounded UDP media sender tests.

use std::thread;

use sonimesh_network::{UdpMediaReceiver, UdpMediaSender, UdpSendError};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

fn loopback_sender() -> UdpMediaSender {
    UdpMediaSender::bind("127.0.0.1:0".parse().unwrap()).unwrap()
}

fn loopback_receiver() -> UdpMediaReceiver {
    UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap()).unwrap()
}

fn poll_len(receiver: &mut UdpMediaReceiver) -> usize {
    for _ in 0..1_000 {
        if let Some(datagram) = receiver.poll().unwrap() {
            return datagram.payload.len();
        }
        thread::yield_now();
    }
    panic!("loopback datagram was not delivered");
}

#[test]
fn sends_the_protocol_maximum_payload_unchanged() {
    let sender = loopback_sender();
    let mut receiver = loopback_receiver();
    let payload = [0xA5; MAX_MEDIA_DATAGRAM_LEN];

    assert!(
        sender
            .send_to(receiver.local_addr().unwrap(), &payload)
            .unwrap()
    );
    assert_eq!(poll_len(&mut receiver), MAX_MEDIA_DATAGRAM_LEN);
}

#[test]
fn rejects_a_datagram_above_the_protocol_limit_before_the_socket() {
    let sender = loopback_sender();
    let mut receiver = loopback_receiver();
    let oversized = [0x5A; MAX_MEDIA_DATAGRAM_LEN + 1];

    assert!(matches!(
        sender.send_to(receiver.local_addr().unwrap(), &oversized),
        Err(UdpSendError::DatagramTooLarge)
    ));
    assert_eq!(receiver.poll().unwrap(), None);
}

#[test]
fn source_endpoint_is_reported_by_the_receiver() {
    let sender = loopback_sender();
    let mut receiver = loopback_receiver();
    let source = sender.local_addr().unwrap();

    sender
        .send_to(receiver.local_addr().unwrap(), &[1, 2, 3])
        .unwrap();

    for _ in 0..1_000 {
        if let Some(datagram) = receiver.poll().unwrap() {
            assert_eq!(datagram.payload, [1, 2, 3]);
            assert_eq!(datagram.source, source);
            return;
        }
        thread::yield_now();
    }
    panic!("loopback datagram was not delivered");
}
