//! Deterministic property coverage for the incremental control framing boundary.
//!
//! The untrusted control stream is the pre-authentication framing boundary:
//! reads must decode every length-prefixed frame exactly once and in wire
//! order regardless of how the transport fragments bytes, a frame must never
//! be decoded before its last byte is fed, the reader must never consume bytes
//! beyond the current frame, and writes must reproduce the exact frame bytes
//! through a socket that accepts only small slices at a time. This test drives
//! a fixed pseudo-random interleaving of feeds, polls, and flushes against an
//! independent wire-prefix model and asserts every observable framing
//! invariant.

use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::rc::Rc;

use sonimesh_network::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlError, Envelope, ErrorCode, Hello, StartStream,
    StopStream, StreamState, StreamStatus, encode_control_frame, envelope,
};

const FRAME_COUNT: usize = 200;
const READ_CHUNK: usize = 7;
const MAX_FEED_CHUNK: usize = 24;

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
}

/// One valid control envelope; the payload kind and display name vary so the
/// encoded frames have distinct sizes and request ids.
fn envelope(kind: usize, request_id: u64) -> Envelope {
    let payload = match kind % 5 {
        0 => envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "s".repeat(kind % 16),
        }),
        1 => envelope::Payload::StartStream(StartStream {
            media_session_id: 1,
            stream_id: 1,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0,
        }),
        2 => envelope::Payload::StopStream(StopStream {
            media_session_id: 1,
            stream_id: 1,
            media_key_epoch: 0,
        }),
        3 => envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: 1,
            stream_id: 1,
            state: StreamState::Streaming as i32,
            detail: String::new(),
        }),
        _ => envelope::Payload::Error(ControlError {
            code: ErrorCode::InvalidRequest as i32,
            retryable: false,
            detail: String::new(),
        }),
    };
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(payload),
    }
}

/// Reads the big-endian payload length at a wire cursor.
fn parse_length(wire: &[u8], cursor: usize) -> usize {
    let bytes: [u8; 4] = wire[cursor..cursor + 4]
        .try_into()
        .expect("length prefix is available");
    usize::try_from(u32::from_be_bytes(bytes)).unwrap_or(usize::MAX)
}

/// In-memory non-blocking byte source the test feeds in random chunks. The
/// probe clone shares state with the reader so the test can observe how many
/// bytes were fed and consumed and when the source is closed.
#[derive(Clone)]
struct FeedStream {
    state: Rc<RefCell<FeedState>>,
    chunk: usize,
}

struct FeedState {
    wire: Vec<u8>,
    offset: usize,
    eof: bool,
}

impl FeedStream {
    fn new(chunk: usize) -> Self {
        Self {
            state: Rc::new(RefCell::new(FeedState {
                wire: Vec::new(),
                offset: 0,
                eof: false,
            })),
            chunk,
        }
    }

    fn push(&self, bytes: &[u8]) {
        self.state.borrow_mut().wire.extend_from_slice(bytes);
    }

    fn fed(&self) -> usize {
        self.state.borrow().wire.len()
    }

    fn consumed(&self) -> usize {
        self.state.borrow().offset
    }

    fn close(&self) {
        self.state.borrow_mut().eof = true;
    }
}

impl Read for FeedStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut state = self.state.borrow_mut();
        if state.offset < state.wire.len() {
            let count = (state.wire.len() - state.offset)
                .min(self.chunk)
                .min(buf.len());
            buf[..count].copy_from_slice(&state.wire[state.offset..state.offset + count]);
            state.offset += count;
            Ok(count)
        } else if state.eof {
            Ok(0)
        } else {
            Err(ErrorKind::WouldBlock.into())
        }
    }
}

impl Write for FeedStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// In-memory write-only sink that accepts at most `chunk` bytes per write so
/// the writer sees realistic socket backpressure.
#[derive(Clone)]
struct SinkStream {
    state: Rc<RefCell<SinkState>>,
}

struct SinkState {
    received: Vec<u8>,
    chunk: usize,
}

impl SinkStream {
    fn new(chunk: usize) -> Self {
        Self {
            state: Rc::new(RefCell::new(SinkState {
                received: Vec::new(),
                chunk,
            })),
        }
    }

    fn set_chunk(&self, chunk: usize) {
        self.state.borrow_mut().chunk = chunk;
    }

    fn received(&self) -> Vec<u8> {
        self.state.borrow().received.clone()
    }
}

impl Write for SinkStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = self.state.borrow_mut();
        let count = buf.len().min(state.chunk);
        state.received.extend_from_slice(&buf[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Read for SinkStream {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(ErrorKind::WouldBlock.into())
    }
}

/// Independent writer model that predicts send and flush outcomes from the
/// sink's per-write acceptance size and verifies the exact received byte
/// prefix, without sharing any state machine with the implementation.
struct WriterModel {
    chunk: usize,
    accepted: usize,
    pending: Option<(usize, usize)>,
}

impl WriterModel {
    fn new(chunk: usize) -> Self {
        Self {
            chunk,
            accepted: 0,
            pending: None,
        }
    }

    fn send(&mut self, frame_len: usize) -> Result<UntrustedControlWrite, ()> {
        if self.pending.is_some() {
            return Err(());
        }
        let accept = frame_len.min(self.chunk);
        self.accepted += accept;
        if accept == frame_len {
            Ok(UntrustedControlWrite::Complete)
        } else {
            self.pending = Some((frame_len, accept));
            Ok(UntrustedControlWrite::Pending)
        }
    }

    fn flush_write(&mut self) -> UntrustedControlWrite {
        let Some((frame_len, written)) = self.pending else {
            return UntrustedControlWrite::Complete;
        };
        let accept = (frame_len - written).min(self.chunk);
        let written = written + accept;
        self.accepted += accept;
        if written == frame_len {
            self.pending = None;
            UntrustedControlWrite::Complete
        } else {
            self.pending = Some((frame_len, written));
            UntrustedControlWrite::Pending
        }
    }
}

/// `feed`/`fed`/`reader`/`writer`/`probe`/`model` are short local names, not a
/// design smell, so the noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn random_chunked_reads_decode_frames_in_wire_order() {
    let mut rng = XorShift64(0xCAFE_BABE_2026_0003);
    let mut request_id = 100_u64;
    let mut envelopes = Vec::with_capacity(FRAME_COUNT);
    let mut wire = Vec::new();
    for kind in 0..FRAME_COUNT {
        let frame = envelope(kind, request_id);
        request_id += 1;
        wire.extend_from_slice(&encode_control_frame(&frame).unwrap());
        envelopes.push(frame);
    }

    let feed = FeedStream::new(READ_CHUNK);
    let probe = feed.clone();
    let mut reader = UntrustedControlStream::from_io(feed);
    let mut fed = 0_usize;
    let mut frames_decoded = 0_usize;
    let mut empty_polls = 0_usize;
    let mut one_byte_feeds = 0_usize;
    let mut model_cursor = 0_usize;

    while frames_decoded < FRAME_COUNT {
        assert!(
            frames_decoded + empty_polls < 1_000_000,
            "reader made no progress"
        );
        if fed < wire.len() && rng.next_u32() % 4 != 0 {
            let chunk = 1 + rng.next_usize(MAX_FEED_CHUNK);
            if chunk == 1 {
                one_byte_feeds += 1;
            }
            let count = chunk.min(wire.len() - fed);
            probe.push(&wire[fed..fed + count]);
            fed += count;
        }

        let cursor_before = model_cursor;
        match reader.poll().unwrap() {
            UntrustedControlPoll::Frame(frame) => {
                let declared = parse_length(&wire, cursor_before);
                let frame_end = cursor_before + 4 + declared;
                assert!(
                    probe.fed() >= frame_end,
                    "frame {frames_decoded} decoded before its bytes were fed"
                );
                assert_eq!(
                    probe.consumed(),
                    frame_end,
                    "frame {frames_decoded} consumption must equal one frame"
                );
                assert_eq!(
                    &frame, &envelopes[frames_decoded],
                    "frame {frames_decoded} content mismatch"
                );
                model_cursor = frame_end;
                frames_decoded += 1;
            }
            UntrustedControlPoll::Empty => {
                empty_polls += 1;
                let consumed = probe.consumed();
                assert!(
                    consumed >= model_cursor,
                    "reader fell behind the frame boundary"
                );
                assert!(consumed <= probe.fed(), "reader consumed unfed bytes");
                if consumed >= model_cursor + 4 {
                    let declared = parse_length(&wire, model_cursor);
                    assert!(
                        consumed <= model_cursor + 4 + declared,
                        "reader over-read into the next frame"
                    );
                }
            }
            UntrustedControlPoll::Closed => {
                panic!("reader closed before all frames decoded")
            }
        }
    }

    assert!(
        empty_polls > 0,
        "the fixed sequence mix must exercise partial-read Empty polls"
    );
    assert!(
        one_byte_feeds > 0,
        "the fixed sequence mix must split frames at byte boundaries"
    );

    probe.close();
    assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
    assert_eq!(reader.poll().unwrap(), UntrustedControlPoll::Closed);
}

/// `writer`/`probe`/`model`/`expected` are short local names, not a design
/// smell, so the noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn chunked_writes_produce_exact_frame_bytes() {
    let mut request_id = 2_000_u64;
    let mut envelopes = Vec::new();
    let mut frames = Vec::new();
    for kind in 0..60 {
        let frame = envelope(kind, request_id);
        request_id += 1;
        frames.push(encode_control_frame(&frame).unwrap());
        envelopes.push(frame);
    }
    let expected = frames.concat();

    let sink = SinkStream::new(3);
    let probe = sink.clone();
    let mut writer = UntrustedControlStream::from_io(sink);
    let mut model = WriterModel::new(3);
    let mut complete_on_send = 0_usize;
    let mut pending_starts = 0_usize;
    let mut pending_errors = 0_usize;
    let mut pending_flushes = 0_usize;

    // Small chunk: every frame needs several flush calls. Large chunk: every
    // send completes in one write.
    for (chunk, start, end) in [
        (3_usize, 0_usize, 40_usize),
        (4096_usize, 40_usize, 60_usize),
    ] {
        probe.set_chunk(chunk);
        model.chunk = chunk;
        for index in start..end {
            let frame_len = frames[index].len();
            let real = writer.send(&envelopes[index]);
            let model_outcome = model.send(frame_len);
            match (real, model_outcome) {
                (Ok(UntrustedControlWrite::Complete), Ok(UntrustedControlWrite::Complete)) => {
                    complete_on_send += 1;
                }
                (Ok(UntrustedControlWrite::Pending), Ok(UntrustedControlWrite::Pending)) => {
                    pending_starts += 1;
                    assert!(matches!(
                        writer.send(&envelopes[index]),
                        Err(UntrustedControlError::WritePending)
                    ));
                    assert!(model.send(frame_len).is_err());
                    pending_errors += 1;
                    loop {
                        assert!(pending_flushes < 1_000_000, "flush made no progress");
                        let real_flush = writer.flush_write().unwrap();
                        let model_flush = model.flush_write();
                        assert_eq!(real_flush, model_flush, "flush outcome mismatch");
                        pending_flushes += 1;
                        if real_flush == UntrustedControlWrite::Complete {
                            break;
                        }
                    }
                }
                (real_outcome, model_outcome) => {
                    panic!("send mismatch: real {real_outcome:?}, model {model_outcome:?}")
                }
            }
            assert_eq!(
                probe.received().as_slice(),
                &expected[..model.accepted],
                "received byte prefix drift after frame {index}"
            );
        }
    }

    assert!(
        complete_on_send > 0,
        "large-chunk sends must complete directly"
    );
    assert!(
        pending_starts > 0,
        "small-chunk sends must report backpressure"
    );
    assert!(
        pending_errors > 0,
        "second sends while pending must be rejected"
    );
    assert!(pending_flushes > 0, "pending frames must require flushes");
    assert_eq!(probe.received(), expected, "final byte stream mismatch");
}
