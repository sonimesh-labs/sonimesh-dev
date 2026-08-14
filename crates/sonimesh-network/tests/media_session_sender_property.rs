//! Deterministic property coverage for the bounded media session sender.
//!
//! `MediaSessionSender` is the session-level sender boundary for one stream and
//! key epoch: it owns the per-epoch sealer (sequence and AEAD), the
//! non-blocking socket, and the routing destination. Rotating to a strictly
//! newer key epoch replaces the sealer and starts a fresh sequence at zero,
//! mirroring the receiver pipeline's rotation boundary. This test drives
//! randomized sends, oversized-frame rejection, key-epoch rotation (valid and
//! fail-closed), fresh-session installation, and routing migration over real
//! loopback sockets against an independent sequence/context model, asserting
//! after every step that the sender context matches the model, that every
//! accepted seal consumed exactly the next per-epoch sequence and reached the
//! current peer's protected ingress with the expected header and plaintext,
//! that an oversized frame is rejected before touching the socket without
//! consuming a sequence, and that failed rotation or fresh-session replacement
//! leaves the model-visible state unchanged or exactly as installed.

use std::thread;

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
const KEY_BYTE: u8 = 0xA5;

fn context(session_id: u64, stream_id: u32, epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        session_id,
        stream_id,
        epoch,
        MediaDirection::AndroidToDesktop,
    )
    .expect("fixed test context is valid")
}

fn secret() -> SessionSecret {
    SessionSecret::new([KEY_BYTE; 32])
}

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

    fn next_u8(&mut self) -> u8 {
        (self.next_u32() >> 24) as u8
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
}

/// Sender plus a loopback receiver pair and the matching protected ingress.
///
/// `model_context` and `next_sequence` are the independent projection of the
/// sender's sealer state; every accepted send must consume exactly
/// `next_sequence` and every rotation must either install the target context
/// and reset the sequence to zero or fail closed without changing either.
struct SenderHarness {
    sender: MediaSessionSender,
    receiver_a: UdpMediaReceiver,
    receiver_b: UdpMediaReceiver,
    ingress: ProtectedMediaIngress,
    model_context: MediaKeyContext,
    next_sequence: u32,
    peer_on_b: bool,
    frames_sent: u64,
}

impl SenderHarness {
    fn new() -> Self {
        let sender = MediaSessionSender::bind(
            "127.0.0.1:0".parse().unwrap(),
            &secret(),
            context(SESSION_ID, STREAM_ID, BASE_EPOCH),
            "127.0.0.1:9".parse().unwrap(),
        )
        .expect("loopback sender binds");
        let receiver_a = UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap())
            .expect("loopback receiver A binds");
        let receiver_b = UdpMediaReceiver::bind("127.0.0.1:0".parse().unwrap())
            .expect("loopback receiver B binds");
        let model_context = context(SESSION_ID, STREAM_ID, BASE_EPOCH);
        let ingress = ProtectedMediaIngress::new(&secret(), model_context)
            .expect("fixed ingress context is valid");
        let mut harness = Self {
            sender,
            receiver_a,
            receiver_b,
            ingress,
            model_context,
            next_sequence: 0,
            peer_on_b: false,
            frames_sent: 0,
        };
        harness.sender.set_peer(harness.current_peer());
        harness
    }

    fn current_peer(&self) -> std::net::SocketAddr {
        if self.peer_on_b {
            self.receiver_b.local_addr().unwrap()
        } else {
            self.receiver_a.local_addr().unwrap()
        }
    }

    fn reinstall_ingress(&mut self, next: MediaKeyContext) {
        self.ingress =
            ProtectedMediaIngress::new(&secret(), next).expect("replacement ingress is valid");
    }

    /// Drains one loopback datagram from the current peer's receiver and
    /// verifies it through the current protected ingress.
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

    /// Asserts the current peer's receiver has no queued datagrams.
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
        self.frames_sent += 1;
    }

    fn send_oversized(&mut self) {
        let payload = vec![0xA5; MAX_MEDIA_PLAINTEXT_LEN + 1];
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

    /// Rotates to a strictly newer epoch, or exercises a fail-closed path.
    ///
    /// `mode % 3 == 0` performs a valid rotation to `epoch_step` epochs newer;
    /// `mode % 3 == 1` changes the stream id (context mismatch); otherwise the
    /// target epoch is not strictly newer.
    fn rotate(&mut self, mode: u8, epoch_step: u16) {
        let step = if epoch_step == 0 { 1 } else { epoch_step };
        match mode % 3 {
            0 => {
                let target_epoch = self.model_context.key_epoch + step;
                let target = context(
                    self.model_context.session_id,
                    self.model_context.stream_id,
                    target_epoch,
                );
                assert!(target.key_epoch > self.model_context.key_epoch);
                self.sender.rotate(&secret(), target).unwrap();
                self.model_context = target;
                self.next_sequence = 0;
                self.reinstall_ingress(target);
            }
            1 => {
                let mut target = self.model_context;
                target.stream_id += 1;
                assert!(matches!(
                    self.sender.rotate(&secret(), target),
                    Err(MediaSessionSenderError::RotationContextMismatch)
                ));
            }
            _ => {
                let target = self.model_context;
                assert!(matches!(
                    self.sender.rotate(&secret(), target),
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

    /// Installs a fresh session context and restarts the sequence at zero.
    fn fresh_session(&mut self, session_delta: u8, epoch: u16) {
        let session_id = SESSION_ID ^ u64::from(session_delta).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let target = context(session_id, STREAM_ID, epoch);
        self.sender
            .install_fresh_session(&secret(), target)
            .expect("fresh session context is valid");
        self.model_context = target;
        self.next_sequence = 0;
        self.reinstall_ingress(target);
        assert_eq!(self.sender.context(), target);
    }

    /// Migrates routing to the other loopback receiver without rotating keys.
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

#[test]
fn sender_projection_matches_an_independent_model_after_every_step() {
    let mut harness = SenderHarness::new();
    let mut rng = XorShift64(0x9E37_79B9_7F4A_7C15);

    // Deterministic edge coverage: valid sends, oversized rejection, a
    // fail-closed mismatch, a valid rotation, a fresh session, and a routing
    // migration.
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

    // Randomized membership operations.
    for _ in 0..200 {
        match rng.next_usize(6) {
            0 => {
                let mut payload = vec![0_u8; rng.next_usize(64)];
                for byte in &mut payload {
                    *byte = rng.next_u8();
                }
                harness.send_valid(rng.next_u8(), rng.next_u32().into(), &payload);
            }
            1 => harness.send_oversized(),
            2 => harness.rotate(rng.next_u8(), u16::try_from(rng.next_u32() % 4).unwrap()),
            3 => harness.fresh_session(
                rng.next_u8(),
                u16::try_from(rng.next_u32() % 4).unwrap() + 1,
            ),
            4 => harness.toggle_peer(),
            _ => harness.send_valid(0, 0, &[]),
        }
    }

    // A final quiet drain proves no stray datagrams and a consistent model.
    harness.assert_quiet();
    assert_eq!(harness.sender.context(), harness.model_context);
    assert_eq!(
        harness.sender.context(),
        context(
            harness.model_context.session_id,
            harness.model_context.stream_id,
            harness.model_context.key_epoch,
        )
    );
    assert!(harness.frames_sent >= 20, "the walk must send frames");
}
