//! Non-blocking bounded UDP media receiver tests.

use std::net::UdpSocket;
use std::thread;

use sonimesh_network::{UdpMediaReceiver, UdpReceiveError};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

fn loopback_receiver() -> UdpMediaReceiver {
    UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap()).unwrap()
}

fn send_and_poll(
    receiver: &mut UdpMediaReceiver,
    payload: &[u8],
) -> Result<usize, UdpReceiveError> {
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    sender
        .send_to(payload, receiver.local_addr().unwrap())
        .unwrap();

    for _ in 0..1_000 {
        match receiver.poll()? {
            Some(datagram) => return Ok(datagram.payload.len()),
            None => thread::yield_now(),
        }
    }
    panic!("loopback datagram was not delivered");
}

#[test]
fn poll_is_nonblocking_when_no_datagram_is_ready() {
    let mut receiver = loopback_receiver();

    assert_eq!(receiver.poll().unwrap(), None);
}

#[test]
fn accepts_the_protocol_maximum_without_allocating_a_larger_payload() {
    let mut receiver = loopback_receiver();
    let payload = [0xA5; MAX_MEDIA_DATAGRAM_LEN];

    assert_eq!(
        send_and_poll(&mut receiver, &payload).unwrap(),
        MAX_MEDIA_DATAGRAM_LEN
    );
}

#[test]
fn rejects_a_datagram_above_the_protocol_limit_before_parser_ingress() {
    let mut receiver = loopback_receiver();
    let payload = [0x5A; MAX_MEDIA_DATAGRAM_LEN + 1];

    assert!(matches!(
        send_and_poll(&mut receiver, &payload),
        Err(UdpReceiveError::DatagramTooLarge)
    ));
}

#[test]
fn returns_the_untrusted_source_endpoint_with_the_payload() {
    let mut receiver = loopback_receiver();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let destination = receiver.local_addr().unwrap();
    sender.send_to(&[1, 2, 3], destination).unwrap();

    for _ in 0..1_000 {
        if let Some(datagram) = receiver.poll().unwrap() {
            assert_eq!(datagram.payload, [1, 2, 3]);
            assert_eq!(datagram.source, sender.local_addr().unwrap());
            return;
        }
        thread::yield_now();
    }
    panic!("loopback datagram was not delivered");
}

#[test]
fn bind_rejects_an_already_used_local_endpoint() {
    let receiver = loopback_receiver();
    let occupied = receiver.local_addr().unwrap();

    let Err(bind_error) = UdpMediaReceiver::bind(occupied) else {
        panic!("second bind unexpectedly succeeded");
    };
    assert_eq!(bind_error.kind(), std::io::ErrorKind::AddrInUse);
}

#[test]
fn from_socket_switches_to_nonblocking_mode() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut receiver = UdpMediaReceiver::from_socket(socket).unwrap();

    assert_eq!(receiver.poll().unwrap(), None);
}
