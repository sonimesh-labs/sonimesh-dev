//! Symmetric framed control-session tests over one real TCP connection.
//!
//! The framing-level tests in `untrusted_control_stream.rs` keep one end as a
//! raw socket, and the app-level wire tests drive the coordinator from only a
//! framed reader. These tests prove the remaining duplex contract: both ends
//! use the same bounded `UntrustedControlStream` reader/writer over a single
//! connection, frames round-trip in both directions, wire order is preserved
//! independently in each direction, and one side's close retires the peer's
//! reader and writer together.

use std::net::{TcpListener, TcpStream};
use std::thread;

use sonimesh_network::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlError, Envelope, ErrorCode, Hello, StartStream,
    StreamState, StreamStatus, envelope,
};

fn hello_envelope(request_id: u64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "duplex peer".to_owned(),
        })),
    }
}

fn start_envelope(request_id: u64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0x2345,
        })),
    }
}

fn status_envelope(request_id: u64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: 7,
            stream_id: 9,
            state: StreamState::Streaming as i32,
            detail: "receiving".to_owned(),
        })),
    }
}

fn error_envelope(request_id: u64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(envelope::Payload::Error(ControlError {
            code: ErrorCode::InvalidRequest as i32,
            retryable: false,
            detail: "rejected".to_owned(),
        })),
    }
}

/// Builds one real loopback connection with both ends wrapped in the bounded
/// framed control reader/writer.
fn duplex_pair() -> (
    UntrustedControlStream<TcpStream>,
    UntrustedControlStream<TcpStream>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let client = TcpStream::connect(address).unwrap();
    let (server, _) = listener.accept().unwrap();
    (
        UntrustedControlStream::from_stream(client).unwrap(),
        UntrustedControlStream::from_stream(server).unwrap(),
    )
}

/// Queues and flushes one envelope until the socket accepts the whole frame.
fn send_envelope(stream: &mut UntrustedControlStream<TcpStream>, envelope: &Envelope) {
    for _ in 0..10_000 {
        match stream.send(envelope) {
            Ok(UntrustedControlWrite::Complete) => return,
            Ok(UntrustedControlWrite::Pending) | Err(UntrustedControlError::WritePending) => {}
            Ok(UntrustedControlWrite::Closed) => panic!("stream closed while sending"),
            Err(error) => panic!("send failed: {error:?}"),
        }
        match stream.flush_write() {
            Ok(UntrustedControlWrite::Complete) => return,
            Ok(UntrustedControlWrite::Pending) => {}
            Ok(UntrustedControlWrite::Closed) => panic!("stream closed while flushing"),
            Err(error) => panic!("flush failed: {error:?}"),
        }
        thread::yield_now();
    }
    panic!("frame was never accepted by the socket");
}

/// Polls the framed reader until exactly one complete control frame arrives.
fn receive_envelope(stream: &mut UntrustedControlStream<TcpStream>) -> Envelope {
    for _ in 0..10_000 {
        match stream.poll() {
            Ok(UntrustedControlPoll::Frame(envelope)) => return envelope,
            Ok(UntrustedControlPoll::Empty) => {}
            Ok(UntrustedControlPoll::Closed) => panic!("stream closed before a frame arrived"),
            Err(error) => panic!("poll failed: {error:?}"),
        }
        thread::yield_now();
    }
    panic!("no complete control frame arrived");
}

#[test]
fn hello_round_trip_uses_framed_writer_and_reader_on_both_ends() {
    let (mut client, mut server) = duplex_pair();

    // Each framed end observes the other side's real loopback endpoint.
    let client_peer = client.peer_addr().unwrap();
    let server_peer = server.peer_addr().unwrap();
    assert!(client_peer.ip().is_loopback());
    assert_eq!(client_peer.ip(), server_peer.ip());
    assert_ne!(client_peer.port(), server_peer.port());

    let hello = hello_envelope(7);
    send_envelope(&mut client, &hello);
    assert_eq!(receive_envelope(&mut server), hello);

    let status = status_envelope(10);
    send_envelope(&mut server, &status);
    assert_eq!(receive_envelope(&mut client), status);
}

#[test]
fn pipelined_frames_keep_wire_order_independently_in_both_directions() {
    let (mut client, mut server) = duplex_pair();

    let hello = hello_envelope(7);
    let start = start_envelope(8);
    send_envelope(&mut client, &hello);
    send_envelope(&mut client, &start);

    assert_eq!(receive_envelope(&mut server), hello);
    assert_eq!(receive_envelope(&mut server), start);

    let status = status_envelope(10);
    let error = error_envelope(11);
    send_envelope(&mut server, &status);
    send_envelope(&mut server, &error);

    assert_eq!(receive_envelope(&mut client), status);
    assert_eq!(receive_envelope(&mut client), error);
}

#[test]
fn one_side_close_retires_the_peer_reader_and_writer_together() {
    let (mut client, mut server) = duplex_pair();

    let hello = hello_envelope(7);
    send_envelope(&mut client, &hello);
    assert_eq!(receive_envelope(&mut server), hello);

    drop(client);

    for _ in 0..10_000 {
        match server.poll() {
            Ok(UntrustedControlPoll::Closed) => break,
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            Ok(UntrustedControlPoll::Frame(_)) => panic!("frame arrived after peer close"),
            Err(error) => panic!("poll failed after peer close: {error:?}"),
        }
    }
    assert_eq!(server.poll().unwrap(), UntrustedControlPoll::Closed);

    assert_eq!(
        server.send(&status_envelope(10)).unwrap(),
        UntrustedControlWrite::Closed
    );
    assert_eq!(server.flush_write().unwrap(), UntrustedControlWrite::Closed);
}
