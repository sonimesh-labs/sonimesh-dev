#![no_main]

//! Fuzz the bounded media session sender boundary.
//!
//! `MediaSessionSender` is the session-level sender boundary for one stream and
//! key epoch: it owns the per-epoch sealer (sequence and AEAD), the
//! non-blocking socket, and the routing destination. Rotating to a strictly
//! newer key epoch replaces the sealer and starts a fresh sequence at zero,
//! mirroring the receiver pipeline's rotation boundary. This target drives
//! byte-selected sends, oversized-frame rejection, valid and fail-closed
//! key-epoch rotation, fresh-session installation, and routing migration over
//! real loopback sockets, asserting after every step that the sender context
//! matches an independent model, that every accepted seal consumed exactly the
//! next per-epoch sequence and reached the current peer's protected ingress
//! with the expected header and plaintext, that an oversized frame is rejected
//! before touching the socket without consuming a sequence, and that failed
//! rotation or fresh-session replacement leaves the model-visible state
//! unchanged or exactly as installed.
//!
//! The walk mirrors the seeded property test
//! (`crates/sonimesh-network/tests/media_session_sender_property.rs`) so the
//! deterministic and byte-driven paths assert the same contract.

use std::thread;

use libfuzzer_sys::fuzz_target;
use sonimesh_network::{
    MediaSessionSendError, MediaSessionSendOutcome, MediaSessionSender, MediaSessionSenderError,
    ProtectedMediaIngress, UdpMediaReceiver,
};
use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaDirection, MediaHeader, MediaKeyContext, SessionSecret,
};

const SESSION_ID: u64 = 0x0102_0304_0506_0708;
const STREAM_ID: u32 = 9;
const BASE_EPOCH: u16 = 1;

/// Reads one byte from `data`, wrapping around the slice.
fn next_byte(data: &[u8], offset: &mut usize) -> u8 {
    let byte = data[*offset % data.len()];
    *offset += 1;
    byte
}

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = next_byte(data, offset);
    }
    u64::from_le_bytes(bytes)
}

fn context(session_id: u64, stream_id: u32, epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        session_id,
        stream_id,
        epoch,
        MediaDirection::AndroidToDesktop,
    )
    .expect("fixed test context is valid")
}

/// Drives the real sender and asserts the sequence/context projection contract.
struct SenderWalk {
    sender: MediaSessionSender,
    receiver_a: UdpMediaReceiver,
    receiver_b: UdpMediaReceiver,
    ingress: ProtectedMediaIngress,
    model_context: MediaKeyContext,
    next_sequence: u32,
    peer_on_b: bool,
}

impl SenderWalk {
    fn new() -> Self {
        let sender = MediaSessionSender::bind(
            "127.0.0.1:0".parse().unwrap(),
            &SessionSecret::new([0xA5; 32]),
            context(SESSION_ID, STREAM_ID, BASE_EPOCH),
            "127.0.0.1:9".parse().unwrap(),
        )
        .expect("loopback sender binds");
        let receiver_a = UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap())
            .expect("loopback receiver A binds");
        let receiver_b = UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap())
            .expect("loopback receiver B binds");
        let model_context = context(SESSION_ID, STREAM_ID, BASE_EPOCH);
        let ingress = ProtectedMediaIngress::new(&SessionSecret::new([0xA5; 32]), model_context)
            .expect("fixed ingress context is valid");
        let mut walk = Self {
            sender,
            receiver_a,
            receiver_b,
            ingress,
            model_context,
            next_sequence: 0,
            peer_on_b: false,
        };
        walk.sender.set_peer(walk.current_peer());
        walk
    }

    fn current_peer(&self) -> std::net::SocketAddr {
        if self.peer_on_b {
            self.receiver_b.local_addr().unwrap()
        } else {
            self.receiver_a.local_addr().unwrap()
        }
    }

    fn reinstall_ingress(&mut self, next: MediaKeyContext) {
        self.ingress = ProtectedMediaIngress::new(&SessionSecret::new([0xA5; 32]), next)
            .expect("replacement ingress is valid");
    }

    fn receive_next(&mut self) -> (MediaHeader, Vec<u8>) {
        for _ in 0..1_000 {
            let datagram = if self.peer_on_b {
                self.receiver_b.poll().unwrap()
            } else {
                self.receiver_a.poll().unwrap()
            };
            if let Some(datagram) = datagram {
                let mut plaintext = [0_u8; MAX_MEDIA_PLAINTEXT_LEN];
                let frame = self
                    .ingress
                    .receive(datagram.payload, &mut plaintext)
                    .expect("sealed frame must authenticate");
                return (frame.header, frame.opus_payload.to_vec());
            }
            thread::yield_now();
        }
        panic!("loopback datagram was not delivered");
    }

    fn assert_quiet(&mut self) {
        for _ in 0..100 {
            let polled = if self.peer_on_b {
                self.receiver_b.poll().unwrap()
            } else {
                self.receiver_a.poll().unwrap()
            };
            assert_eq!(polled, None, "unexpected datagram reached the current peer");
            thread::yield_now();
        }
    }

    fn send_valid(&mut self, flags: u8, timestamp: u64, payload: &[u8]) {
        let outcome = self.sender.send_frame(flags, timestamp, payload).unwrap();
        assert_eq!(
            outcome,
            MediaSessionSendOutcome::Sent {
                sequence: self.next_sequence,
            },
            "accepted send must consume exactly the next per-epoch sequence"
        );
        let (header, plaintext) = self.receive_next();
        assert_eq!(header.sequence, self.next_sequence);
        assert_eq!(header.session_id, self.model_context.session_id);
        assert_eq!(header.stream_id, self.model_context.stream_id);
        assert_eq!(header.key_epoch, self.model_context.key_epoch);
        assert_eq!(header.flags, flags);
        assert_eq!(header.sample_timestamp, timestamp);
        assert_eq!(plaintext, payload);
        self.next_sequence += 1;
    }

    fn send_oversized(&mut self) {
        let payload = [0xA5_u8; MAX_MEDIA_PLAINTEXT_LEN + 1];
        assert!(matches!(
            self.sender.send_frame(0, 0, &payload),
            Err(MediaSessionSendError::PayloadTooLarge)
        ));
        self.assert_quiet();
        assert_eq!(
            self.sender.context(),
            self.model_context,
            "an oversized frame must not change the sender context"
        );
    }

    fn rotate(&mut self, mode: u8, epoch_step: u16) {
        let step = if epoch_step == 0 { 1 } else { epoch_step };
        match mode % 3 {
            0 => {
                let target = context(
                    self.model_context.session_id,
                    self.model_context.stream_id,
                    self.model_context.key_epoch + step,
                );
                self.sender
                    .rotate(&SessionSecret::new([0xA5; 32]), target)
                    .unwrap();
                self.model_context = target;
                self.next_sequence = 0;
                self.reinstall_ingress(target);
            }
            1 => {
                let mut target = self.model_context;
                target.stream_id += 1;
                assert!(matches!(
                    self.sender.rotate(&SessionSecret::new([0xA5; 32]), target),
                    Err(MediaSessionSenderError::RotationContextMismatch)
                ));
            }
            _ => {
                let target = self.model_context;
                assert!(matches!(
                    self.sender.rotate(&SessionSecret::new([0xA5; 32]), target),
                    Err(MediaSessionSenderError::NonIncreasingEpoch)
                ));
            }
        }
        assert_eq!(
            self.sender.context(),
            self.model_context,
            "rotation must leave the sender context equal to the model"
        );
    }

    fn fresh_session(&mut self, session_delta: u8, epoch: u16) {
        let session_id = SESSION_ID ^ u64::from(session_delta).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let target = context(session_id, STREAM_ID, epoch);
        if session_id == self.model_context.session_id {
            // A byte-derived delta can collide with the active session ID.
            // The no-reuse boundary (ADR 0017) must reject that fail-closed
            // without changing the sender state or consuming a sequence.
            assert!(matches!(
                self.sender
                    .install_fresh_session(&SessionSecret::new([0xA5; 32]), target),
                Err(MediaSessionSenderError::ReusedSessionId)
            ));
            assert_eq!(
                self.sender.context(),
                self.model_context,
                "a rejected fresh session must not change the sender context"
            );
            return;
        }
        self.sender
            .install_fresh_session(&SessionSecret::new([0xA5; 32]), target)
            .expect("fresh session context is valid");
        self.model_context = target;
        self.next_sequence = 0;
        self.reinstall_ingress(target);
        assert_eq!(self.sender.context(), target);
    }

    fn toggle_peer(&mut self) {
        self.peer_on_b = !self.peer_on_b;
        self.sender.set_peer(self.current_peer());
        assert_eq!(
            self.sender.context(),
            self.model_context,
            "routing migration must not change the sender context"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut offset = 0;
    let mut harness = SenderWalk::new();

    // Deterministic edge coverage on every input: valid sends, oversized
    // rejection, a fail-closed mismatch, a valid rotation, a fresh session, and
    // a routing migration.
    harness.send_valid(0, 0, &[1, 2, 3]);
    harness.send_oversized();
    harness.send_valid(0, 960, &[]);
    harness.rotate(1, 1);
    harness.send_valid(0, 1920, &[4]);
    harness.rotate(0, 1);
    harness.send_valid(0, 2880, &[5]);
    harness.fresh_session(3, 1);
    harness.send_valid(0x01, 3840, &[6, 7]);
    harness.toggle_peer();
    harness.send_valid(0, 4800, &[8]);
    harness.rotate(2, 0);
    harness.send_valid(0, 5760, &[9]);

    // Fuzz-derived sender operations.
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        match next_u64(data, &mut offset) % 8 {
            0 => {
                let flags = next_byte(data, &mut offset);
                let timestamp = next_u64(data, &mut offset);
                let mut payload = vec![0_u8; usize::from(next_byte(data, &mut offset)) % 64];
                for byte in &mut payload {
                    *byte = next_byte(data, &mut offset);
                }
                harness.send_valid(flags, timestamp, &payload);
            }
            1 => harness.send_oversized(),
            2 => {
                let mode = next_byte(data, &mut offset);
                let epoch_step = u16::from(next_byte(data, &mut offset)) % 4;
                harness.rotate(mode, epoch_step);
            }
            3 => {
                let session_delta = next_byte(data, &mut offset);
                let epoch = u16::from(next_byte(data, &mut offset)) % 4 + 1;
                harness.fresh_session(session_delta, epoch);
            }
            4 => harness.toggle_peer(),
            5 => harness.send_valid(0, 0, &[]),
            6 => harness.assert_quiet(),
            _ => {}
        }
    }

    // A final quiet drain leaves the projection consistent on every input.
    harness.assert_quiet();
    assert_eq!(harness.sender.context(), harness.model_context);
});
