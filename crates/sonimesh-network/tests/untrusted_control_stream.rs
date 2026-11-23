//! Bounded incremental control framing tests without transport authentication.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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

fn connected_reader() -> (TcpStream, UntrustedControlStream<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let sender = TcpStream::connect(address).unwrap();
    let (receiver, _) = listener.accept().unwrap();
    (
        sender,
        UntrustedControlStream::from_stream(receiver).unwrap(),
    )
}

struct BackpressuredStream {
    socket: TcpStream,
    released: Arc<AtomicBool>,
    wrote_prefix: bool,
}

impl Read for BackpressuredStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.socket.read(buffer)
    }
}

impl Write for BackpressuredStream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.released.load(Ordering::SeqCst) {
            return self.socket.write(buffer);
        }
        if self.wrote_prefix {
            return Err(ErrorKind::WouldBlock.into());
        }
        let written = self.socket.write(&buffer[..buffer.len().min(4)])?;
        self.wrote_prefix = written > 0;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}

fn backpressured_writer() -> (
    TcpStream,
    UntrustedControlStream<BackpressuredStream>,
    Arc<AtomicBool>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let peer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    socket.set_nonblocking(true).unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let writer = UntrustedControlStream::from_io(BackpressuredStream {
        socket,
        released: Arc::clone(&released),
        wrote_prefix: false,
    });
    (peer, writer, released)
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
    let (_peer, mut writer, _released) = backpressured_writer();
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
fn pending_write_resumes_after_the_peer_drains() {
    let (mut peer, mut writer, released) = backpressured_writer();
    let frame = hello_frame();
    let mut completed_sends = 0_u32;

    // The adapter writes a prefix to the real socket and then reports WouldBlock.
    let mut pending = false;
    for _ in 0..100_000 {
        match writer.send(&hello_envelope()).unwrap() {
            UntrustedControlWrite::Complete => completed_sends += 1,
            UntrustedControlWrite::Pending => {
                pending = true;
                break;
            }
            UntrustedControlWrite::Closed => panic!("loopback writer closed unexpectedly"),
        }
    }
    assert!(pending, "loopback socket never applied backpressure");
    released.store(true, Ordering::SeqCst);

    peer.set_nonblocking(true).unwrap();
    let mut drain = [0_u8; 8_192];
    let mut received_bytes = 0_usize;

    // Finish the partially buffered frame while draining the peer.
    let mut resumed = false;
    for _ in 0..100_000 {
        match writer.flush_write() {
            Ok(UntrustedControlWrite::Complete) => {
                resumed = true;
                break;
            }
            Ok(UntrustedControlWrite::Pending) => {}
            Ok(UntrustedControlWrite::Closed) => panic!("writer closed while flushing"),
            Err(error) => panic!("writer flush failed after peer backpressure: {error}"),
        }
        match peer.read(&mut drain) {
            Ok(read) => received_bytes += read,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("peer read failed while resuming the pending frame: {error}"),
        }
        thread::yield_now();
    }
    assert!(
        resumed,
        "pending control frame never completed after peer drain"
    );

    // A fresh frame is accepted and fully flushed once the resume completed.
    let mut fresh_submitted = false;
    let mut fresh_completed = false;
    for _ in 0..100_000 {
        if !fresh_submitted {
            match writer.send(&hello_envelope()) {
                Ok(UntrustedControlWrite::Complete) => {
                    fresh_completed = true;
                    break;
                }
                Ok(UntrustedControlWrite::Pending) => fresh_submitted = true,
                Ok(UntrustedControlWrite::Closed) => {
                    panic!("loopback writer closed while sending a fresh frame")
                }
                Err(error) => panic!("writer rejected a fresh frame after resuming: {error}"),
            }
        }
        match writer.flush_write() {
            Ok(UntrustedControlWrite::Complete) => {
                fresh_completed = true;
                break;
            }
            Ok(UntrustedControlWrite::Pending) => {}
            Ok(UntrustedControlWrite::Closed) => panic!("writer closed while flushing"),
            Err(error) => panic!("writer flush failed for the fresh frame: {error}"),
        }
        match peer.read(&mut drain) {
            Ok(read) => received_bytes += read,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("peer read failed while flushing the fresh frame: {error}"),
        }
        thread::yield_now();
    }
    assert!(
        fresh_completed,
        "writer did not complete a fresh frame after resuming"
    );

    // Closing the writer sends FIN; drain until EOF and verify every accepted
    // frame arrived intact.
    drop(writer);
    loop {
        match peer.read(&mut drain) {
            Ok(0) => break,
            Ok(read) => received_bytes += read,
            Err(error) if error.kind() == ErrorKind::WouldBlock => thread::yield_now(),
            Err(error) => panic!("peer read failed while draining after close: {error}"),
        }
    }
    let expected = usize::try_from(completed_sends + 2).unwrap() * frame.len();
    assert_eq!(received_bytes, expected);
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
fn zero_length_payload_is_rejected_as_a_protocol_error() {
    let (mut sender, mut reader) = connected_reader();
    // A declared payload length of zero skips the payload read and decodes the
    // length prefix itself: it must fail semantic validation and retire the
    // stream instead of yielding an empty frame.
    sender.write_all(&[0, 0, 0, 0]).unwrap();

    for _ in 0..1_000 {
        match reader.poll() {
            Err(UntrustedControlError::Protocol(ControlFrameError::UnsupportedVersion(0))) => {
                assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
                return;
            }
            Ok(UntrustedControlPoll::Empty) => thread::yield_now(),
            Ok(UntrustedControlPoll::Frame(_)) => panic!("empty frame was decoded"),
            Ok(UntrustedControlPoll::Closed) => panic!("reader closed before protocol error"),
            Err(other) => panic!("unexpected reader error: {other:?}"),
        }
    }
    panic!("zero-length control payload was never rejected");
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

/// In-memory byte source that yields at most `chunk` bytes per read so the
/// bounded framing state machine is exercised across arbitrary chunk boundaries.
struct ChunkedBytes {
    data: Vec<u8>,
    offset: usize,
    chunk: usize,
}

impl ChunkedBytes {
    fn new(data: Vec<u8>, chunk: usize) -> Self {
        Self {
            data,
            offset: 0,
            chunk,
        }
    }
}

impl Read for ChunkedBytes {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.offset >= self.data.len() {
            return Ok(0);
        }
        let count = (self.data.len() - self.offset)
            .min(self.chunk)
            .min(buf.len());
        buf[..count].copy_from_slice(&self.data[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

impl Write for ChunkedBytes {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Drains a chunked reader until closure or error, returning decoded request IDs.
fn drain_frames(
    stream: &mut UntrustedControlStream<ChunkedBytes>,
) -> (Vec<u64>, Option<UntrustedControlError>) {
    let mut request_ids = Vec::new();
    let mut error = None;
    for _ in 0..4096 {
        match stream.poll() {
            Ok(UntrustedControlPoll::Empty) => {}
            Ok(UntrustedControlPoll::Frame(envelope)) => request_ids.push(envelope.request_id),
            Ok(UntrustedControlPoll::Closed) => break,
            Err(failure) => {
                error = Some(failure);
                break;
            }
        }
    }
    (request_ids, error)
}

#[test]
fn in_memory_one_byte_chunks_decode_the_hello_frame() {
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes::new(hello_frame(), 1));

    let (request_ids, error) = drain_frames(&mut stream);

    assert!(error.is_none());
    assert_eq!(request_ids, vec![7]);
}

#[test]
fn in_memory_chunked_oversized_length_is_rejected_and_retires() {
    let length = u32::try_from(MAX_CONTROL_FRAME_BYTES + 1).unwrap();
    let mut stream =
        UntrustedControlStream::from_io(ChunkedBytes::new(length.to_be_bytes().to_vec(), 1));

    let (_, error) = drain_frames(&mut stream);

    assert!(matches!(
        error,
        Some(UntrustedControlError::Protocol(
            ControlFrameError::FrameTooLarge
        ))
    ));
    assert_eq!(stream.poll().unwrap(), UntrustedControlPoll::Closed);
}

#[test]
fn in_memory_chunked_zero_length_payload_is_rejected() {
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes::new(vec![0, 0, 0, 0], 1));

    let (_, error) = drain_frames(&mut stream);

    assert!(matches!(
        error,
        Some(UntrustedControlError::Protocol(
            ControlFrameError::UnsupportedVersion(0)
        ))
    ));
    assert_eq!(stream.poll().unwrap(), UntrustedControlPoll::Closed);
}

#[test]
fn in_memory_eof_mid_payload_reports_closed_without_a_frame() {
    let frame = hello_frame();
    let truncated = frame[..=CONTROL_LENGTH_BYTES].to_vec();
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes::new(truncated, 2));

    let (request_ids, error) = drain_frames(&mut stream);

    assert!(error.is_none());
    assert!(request_ids.is_empty());
    assert_eq!(stream.poll().unwrap(), UntrustedControlPoll::Closed);
}

#[test]
fn in_memory_consecutive_frames_decode_across_variable_chunks() {
    let mut data = hello_frame();
    data.extend_from_slice(&hello_frame());
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes::new(data, 7));

    let (request_ids, error) = drain_frames(&mut stream);

    assert!(error.is_none());
    assert_eq!(request_ids, vec![7, 7]);
}

#[test]
fn in_memory_writer_completes_a_frame_through_the_generic_constructor() {
    let mut stream = UntrustedControlStream::from_io(ChunkedBytes::new(Vec::new(), 1));

    assert_eq!(
        stream.send(&hello_envelope()).unwrap(),
        UntrustedControlWrite::Complete
    );
    assert_eq!(
        stream.flush_write().unwrap(),
        UntrustedControlWrite::Complete
    );
}
