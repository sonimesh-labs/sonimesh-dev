//! Bounded incremental control framing tests without transport authentication.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;

use sonimesh_network::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
const CONTROL_LENGTH_BYTES: usize = 4;

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlFrameError, Envelope, Hello,
    MAX_CONTROL_FRAME_BYTES, encode_control_frame, envelope,
};

fn hello_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 7,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "loopback".to_owned(),
        })),
    }
}

fn hello_frame() -> Vec<u8> {
    encode_control_frame(&hello_envelope()).unwrap()
}

fn connected_reader() -> (TcpStream, UntrustedControlStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let sender = TcpStream::connect(address).unwrap();
    let (receiver, _) = listener.accept().unwrap();
    (
        sender,
        UntrustedControlStream::from_stream(receiver).unwrap(),
    )
}

#[test]
fn fragmented_frame_is_held_until_complete_and_then_validated() {
    let (mut sender, mut reader) = connected_reader();
    let frame = hello_frame();
    let split = frame.len() / 2;
    sender.write_all(&frame[..split]).unwrap();

    for _ in 0..1_000 {
        if matches!(reader.poll().unwrap(), UntrustedControlPoll::Empty) {
            break;
        }
        thread::yield_now();
    }
    sender.write_all(&frame[split..]).unwrap();

    for _ in 0..1_000 {
        match reader.poll().unwrap() {
            UntrustedControlPoll::Empty => thread::yield_now(),
            UntrustedControlPoll::Frame(envelope) => {
                assert_eq!(envelope.request_id, 7);
                return;
            }
            UntrustedControlPoll::Closed => panic!("reader closed before complete frame"),
        }
    }
    panic!("complete control frame was not decoded");
}

#[test]
fn writer_uses_the_same_validated_length_prefixed_frame() {
    let (mut peer, mut writer) = connected_reader();
    let expected = hello_frame();

    assert_eq!(
        writer.send(&hello_envelope()).unwrap(),
        UntrustedControlWrite::Complete
    );
    let mut actual = vec![0; expected.len()];
    peer.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn invalid_outbound_frame_does_not_retire_or_block_the_writer() {
    let (mut peer, mut writer) = connected_reader();
    let mut invalid = hello_envelope();
    invalid.protocol_version = CONTROL_PROTOCOL_VERSION + 1;

    assert!(matches!(
        writer.send(&invalid),
        Err(UntrustedControlError::Protocol(
            ControlFrameError::UnsupportedVersion(version)
        )) if version == invalid.protocol_version
    ));
    assert_eq!(
        writer.send(&hello_envelope()).unwrap(),
        UntrustedControlWrite::Complete
    );

    let expected = hello_frame();
    let mut actual = vec![0; expected.len()];
    peer.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn writer_rejects_a_second_frame_while_the_first_is_pending() {
    let (_peer, mut writer) = connected_reader();
    let mut pending = false;

    for _ in 0..100_000 {
        match writer.send(&hello_envelope()).unwrap() {
            UntrustedControlWrite::Complete => {}
            UntrustedControlWrite::Pending => {
                pending = true;
                break;
            }
            UntrustedControlWrite::Closed => panic!("loopback writer closed unexpectedly"),
        }
    }

    assert!(pending, "loopback socket never applied backpressure");
    assert!(matches!(
        writer.send(&hello_envelope()),
        Err(UntrustedControlError::WritePending)
    ));
}

#[test]
fn permanent_write_error_retires_the_entire_stream() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let socket = TcpStream::connect(address).unwrap();
    let (_peer, _) = listener.accept().unwrap();
    socket.shutdown(Shutdown::Write).unwrap();
    let mut stream = UntrustedControlStream::from_stream(socket).unwrap();

    assert!(matches!(
        stream.send(&hello_envelope()),
        Err(UntrustedControlError::Io(_))
    ));
    assert_eq!(
        stream.send(&hello_envelope()).unwrap(),
        UntrustedControlWrite::Closed
    );
    assert_eq!(stream.flush_write().unwrap(), UntrustedControlWrite::Closed);
    assert_eq!(stream.poll().unwrap(), UntrustedControlPoll::Closed);
}

#[test]
fn consecutive_frames_are_decoded_one_at_a_time() {
    let (mut sender, mut reader) = connected_reader();
    let first = hello_frame();
    let second = hello_frame();
    sender.write_all(&first).unwrap();
    sender.write_all(&second).unwrap();

    let mut decoded = 0;
    for _ in 0..1_000 {
        match reader.poll().unwrap() {
            UntrustedControlPoll::Empty => thread::yield_now(),
            UntrustedControlPoll::Frame(envelope) => {
                assert_eq!(envelope.request_id, 7);
                decoded += 1;
                if decoded == 2 {
                    return;
                }
            }
            UntrustedControlPoll::Closed => panic!("reader closed before both frames"),
        }
    }
    panic!("consecutive control frames were not decoded");
}

#[test]
fn malformed_protobuf_terminates_the_reader_after_protocol_error() {
    let (mut sender, mut reader) = connected_reader();
    sender.write_all(&[0, 0, 0, 1, 0xFF]).unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(sonimesh_protocol::ControlFrameError::Decode(
                _,
            ))) => {
                assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            other => panic!("unexpected malformed-frame result: {other:?}"),
        }
    }
    panic!("malformed control frame was not rejected");
}

#[test]
fn read_protocol_error_retires_the_write_direction() {
    let (mut sender, mut reader) = connected_reader();
    sender.write_all(&[0, 0, 0, 1, 0xFF]).unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(ControlFrameError::Decode(_))) => {
                assert_eq!(
                    reader.send(&hello_envelope()).unwrap(),
                    UntrustedControlWrite::Closed
                );
                assert_eq!(reader.flush_write().unwrap(), UntrustedControlWrite::Closed);
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            Ok(UntrustedControlPoll::Frame(_)) => panic!("malformed frame was decoded"),
            Ok(UntrustedControlPoll::Closed) => panic!("reader closed before protocol error"),
            Err(other) => panic!("unexpected reader error: {other:?}"),
        }
    }
    panic!("malformed control frame was not rejected");
}

#[test]
fn retired_reader_never_decodes_later_valid_frames() {
    let (mut sender, mut reader) = connected_reader();
    // A valid frame already buffered behind the malformed one must never be
    // decoded once the read-side protocol error retires the stream.
    sender.write_all(&[0, 0, 0, 1, 0xFF]).unwrap();
    sender.write_all(&hello_frame()).unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(ControlFrameError::Decode(_))) => {
                for _ in 0..10 {
                    assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                }
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            Ok(UntrustedControlPoll::Frame(_)) => panic!("retired reader decoded a later frame"),
            Ok(UntrustedControlPoll::Closed) => panic!("reader closed before protocol error"),
            Err(other) => panic!("unexpected reader error: {other:?}"),
        }
    }
    panic!("malformed control frame was not rejected");
}

#[test]
fn oversized_length_terminates_the_untrusted_reader() {
    let (mut sender, mut reader) = connected_reader();
    let length = u32::try_from(MAX_CONTROL_FRAME_BYTES + 1).unwrap();
    sender.write_all(&length.to_be_bytes()).unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(
                sonimesh_protocol::ControlFrameError::FrameTooLarge,
            )) => {
                assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            other => panic!("unexpected framing result: {other:?}"),
        }
    }
    panic!("oversized control length was not rejected");
}

#[test]
fn four_byte_payload_is_read_and_rejected_as_protocol_not_close() {
    let (mut sender, mut reader) = connected_reader();
    // Length-prefixed frame whose Protobuf payload is exactly four bytes:
    // version=1, request_id=1, but no oneof payload.
    sender
        .write_all(&[0, 0, 0, 4, 0x08, 0x01, 0x10, 0x01])
        .unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(ControlFrameError::MissingPayload)) => {
                assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            other => panic!("unexpected framing result: {other:?}"),
        }
    }
    panic!("four-byte control payload was never validated");
}

#[test]
fn peer_close_is_reported_without_treating_partial_data_as_a_frame() {
    let (mut sender, mut reader) = connected_reader();
    sender.write_all(&[0, 0]).unwrap();
    drop(sender);

    for _ in 0..1_000 {
        match reader.poll().unwrap() {
            UntrustedControlPoll::Empty => thread::yield_now(),
            UntrustedControlPoll::Closed => return,
            UntrustedControlPoll::Frame(_) => panic!("partial frame was decoded"),
        }
    }
    panic!("peer close was not observed");
}

#[test]
fn peer_close_after_a_complete_length_never_decodes_a_partial_payload() {
    let (mut sender, mut reader) = connected_reader();
    let frame = hello_frame();
    let partial_payload_end = CONTROL_LENGTH_BYTES + 1;
    sender.write_all(&frame[..partial_payload_end]).unwrap();
    drop(sender);

    for _ in 0..1_000 {
        match reader.poll().unwrap() {
            UntrustedControlPoll::Empty => thread::yield_now(),
            UntrustedControlPoll::Closed => {
                assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                return;
            }
            UntrustedControlPoll::Frame(_) => panic!("partial payload was decoded"),
        }
    }
    panic!("peer close after complete length was not observed");
}
