#![no_main]

//! Fuzz the UDP-backed protected receiver composition.
//!
//! `UdpReceiverPipeline` is the application-domain composition of the bounded
//! UDP socket, the fail-closed source pin, the protected ingress, and the
//! decode worker. The source pin must bind only to the endpoint of the first
//! successful protected outcome, a changed endpoint must be rejected before
//! any protected work and never move the pin, failed processing must never
//! bind an unbound source, and only a fresh authenticated session clears the
//! binding. This target drives byte-selected deliveries, rotations, fresh
//! sessions, tampered ciphertext with a valid-twin probe, duplicates,
//! retired-epoch packets, and oversized datagrams over real loopback sockets
//! against an independent pin, replay, and counter model, asserting every
//! poll outcome, pin state, decoded PCM, and diagnostic counter after every
//! step.
//!
//! The model mirrors the seeded property walk
//! (`crates/sonimesh-app/tests/udp_receiver_pipeline_property.rs`) so the
//! deterministic and byte-driven paths assert the same contract.

use core::convert::Infallible;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::thread;

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    AdaptiveTargetConfig, ReceiverPipelineError, UdpPollOutcome, UdpReceiverError,
    UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, JitterMode, JitterTargetConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{IngressError, UdpReceiveError};
use sonimesh_protocol::{
    MAX_MEDIA_DATAGRAM_LEN, MediaCryptoError, MediaDirection, MediaKeyContext, MediaSealer,
    ReplayDecision, ReplayWindow, SessionSecret,
};

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

const SESSION_IDS: [u64; 2] = [0x0102_0304_0506_0708, 0x0A0B_0C0D_0E0F_0102];

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

/// One-byte stateless decoder so authenticated payload bytes map to PCM.
struct ByteDecoder;

impl FrameDecoder for ByteDecoder {
    type Error = Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = f32::from(encoded[0]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }
}

fn context(session_id: u64, epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(session_id, 9, epoch, MediaDirection::AndroidToDesktop).unwrap()
}

/// Independent cumulative counter model for the pipeline's diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CounterModel {
    authenticated: u64,
    duplicate: u64,
    rejected: u64,
    jitter_resync_dropped_frames: u64,
    stream_resets: u64,
}

/// Sends one datagram and polls until the loopback delivery is consumed.
fn send_and_poll(
    sender: &UdpSocket,
    pipeline: &mut Pipeline,
    payload: &[u8],
) -> Result<UdpPollOutcome, UdpReceiverError<Infallible>> {
    sender
        .send_to(payload, pipeline.local_addr().unwrap())
        .unwrap();
    for _ in 0..1_000 {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => thread::yield_now(),
            result => return result,
        }
    }
    panic!("loopback datagram was not delivered");
}

/// Independent pin, replay, and counter model driven against the real
/// pipeline. The real source pin binds on the first protected outcome, so the
/// model tracks the pinned endpoint, a separate replay window for the sealer
/// side, and the four published diagnostic counters.
#[allow(clippy::struct_field_names)]
struct UdpWalk {
    pipeline: Pipeline,
    sender_a: UdpSocket,
    sender_b: UdpSocket,
    source_a: SocketAddr,
    consumer: RealtimePcmConsumer<4>,
    sealer: MediaSealer,
    secret: SessionSecret,
    session_id: u64,
    epoch: u16,
    pinned: Option<SocketAddr>,
    replay: ReplayWindow,
    counters: CounterModel,
    next_expected: u32,
    admitted_seqs: Vec<u32>,
    admitted_datagrams: Vec<Vec<u8>>,
    retired_epoch_packet: Option<Vec<u8>>,
    queue_epoch_dirty: bool,
    step: usize,
}

impl UdpWalk {
    fn new(data: &[u8]) -> Self {
        let secret = SessionSecret::new([0xA5; 32]);
        let session_id = SESSION_IDS[0];
        let epoch = 2_u16;
        let context = context(session_id, epoch);
        let sealer = MediaSealer::new(&secret, context).unwrap();
        let (producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let tick_bytes = data.to_vec();
        let mut tick_index = 0_usize;
        let mut arrival_ticks = 0_u64;
        let tick_source = Box::new(move || {
            let delta = u64::from(tick_bytes[tick_index % tick_bytes.len()]).saturating_add(1);
            tick_index += 1;
            arrival_ticks = arrival_ticks.saturating_add(delta);
            arrival_ticks
        });
        let pipeline = Pipeline::bind_with_adaptive_target(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            ByteDecoder,
            producer,
            Arc::clone(&counters),
            0,
            AdaptiveTargetConfig {
                target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 4),
                arrival_ticks_per_second: 1_000,
                tick_source,
            },
        )
        .unwrap();
        let sender_a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender_b = UdpSocket::bind("127.0.0.1:0").unwrap();
        let source_a = sender_a.local_addr().unwrap();
        Self {
            pipeline,
            sender_a,
            sender_b,
            source_a,
            consumer,
            sealer,
            secret,
            session_id,
            epoch,
            pinned: None,
            replay: ReplayWindow::new(),
            counters: CounterModel::default(),
            next_expected: 0,
            admitted_seqs: Vec::new(),
            admitted_datagrams: Vec::new(),
            retired_epoch_packet: None,
            queue_epoch_dirty: false,
            step: 0,
        }
    }

    /// Asserts the pipeline state matches the independent model and that no
    /// delivered datagram is left unprocessed.
    fn assert_state(&mut self, context: &str) {
        assert!(
            matches!(self.pipeline.poll().unwrap(), UdpPollOutcome::Empty),
            "every delivered datagram must be consumed after {context}"
        );
        assert_eq!(
            self.pipeline.pinned_source(),
            self.pinned,
            "pinned source model drift after {context}"
        );
        assert_eq!(
            self.consumer.active_len(),
            0,
            "decode plus pop keeps the PCM queue empty after {context}"
        );
        let snapshot = self.pipeline.counters().snapshot();
        assert_eq!(
            snapshot.authenticated_packets, self.counters.authenticated,
            "authenticated counter drift after {context}"
        );
        assert_eq!(
            snapshot.duplicate_packets, self.counters.duplicate,
            "duplicate counter drift after {context}"
        );
        assert_eq!(
            snapshot.rejected_packets, self.counters.rejected,
            "rejected counter drift after {context}"
        );
        assert_eq!(
            snapshot.jitter_resync_dropped_frames, self.counters.jitter_resync_dropped_frames,
            "jitter resynchronization drop counter drift after {context}"
        );
        assert_eq!(
            snapshot.stream_resets, self.counters.stream_resets,
            "stream reset counter drift after {context}"
        );
        if snapshot.target_min_frames == 0 {
            assert_eq!(
                (snapshot.target_max_frames, snapshot.target_frames),
                (0, 0),
                "a retired target epoch must publish an empty state after {context}"
            );
            assert_eq!(
                (
                    snapshot.target_smoothed_jitter_ms,
                    snapshot.target_raised_steps,
                    snapshot.target_lowered_steps,
                ),
                (0, 0, 0),
                "a retired target epoch must clear its controller counters after {context}"
            );
        } else {
            assert_eq!(
                (snapshot.target_min_frames, snapshot.target_max_frames),
                (2, 4),
                "the balanced target preset must remain bounded after {context}"
            );
            assert!(
                (2..=4).contains(&snapshot.target_frames),
                "the active target must stay within its water levels after {context}"
            );
            assert!(
                u64::from(snapshot.target_raised_steps) + u64::from(snapshot.target_lowered_steps)
                    <= self.counters.authenticated,
                "target step counters must not exceed authenticated observations after {context}"
            );
        }
    }

    /// Decodes one admitted frame and pops the queued PCM block, asserting the
    /// sequence, sample count, decoded payload byte, and queue discontinuity
    /// flag.
    fn assert_decoded_and_popped(
        &mut self,
        expected_sequence: u32,
        expected_payload: u8,
        expect_discontinuity: bool,
    ) {
        let decoded = self.pipeline.decode_next().unwrap();
        assert!(
            matches!(
                decoded,
                DecodeStep::Decoded {
                    sequence,
                    samples: 1,
                    ..
                } if sequence == expected_sequence
            ),
            "expected decoded sequence {expected_sequence}, got {decoded:?}"
        );
        let mut output = [f32::NAN; 4];
        let popped = self.consumer.pop_into(&mut output);
        assert!(
            matches!(
                popped,
                RealtimePcmPopOutcome::Filled {
                    samples: 1,
                    sequence,
                    discontinuity,
                    discarded_stale_blocks: 0,
                } if sequence == u64::from(expected_sequence)
                    && discontinuity == expect_discontinuity
            ),
            "expected a clean filled block for {expected_sequence}, got {popped:?}"
        );
        assert!(
            (output[0] - f32::from(expected_payload)).abs() < f32::EPSILON,
            "expected payload {expected_payload}, got {}",
            output[0]
        );
    }

    /// Seals, sends, and decodes one fresh packet from the chosen endpoint,
    /// updating the independent pin, replay, and counter models. A changed
    /// endpoint is rejected before protected work and never moves the pin.
    fn deliver_fresh(&mut self, from_b: bool) {
        self.step += 1;
        let sender = if from_b {
            &self.sender_b
        } else {
            &self.sender_a
        };
        let source = sender.local_addr().unwrap();

        if self.pinned.is_some_and(|expected| expected != source) {
            // A different endpoint is rejected before parsing or authentication,
            // so the datagram content is irrelevant and the sealer must not
            // advance.
            let result = send_and_poll(sender, &mut self.pipeline, &[0x42]);
            assert!(
                matches!(result, Err(UdpReceiverError::SourceEndpointChanged)),
                "changed endpoint must be rejected"
            );
            self.counters.rejected += 1;
            return;
        }

        let payload = u8::try_from(self.step % 251).expect("payload stays below 251");
        let packet = self
            .sealer
            .seal(0, u64::from(self.next_expected) * 960, &[payload])
            .unwrap();
        let sequence = packet.header.sequence;

        assert_eq!(
            sequence, self.next_expected,
            "fresh deliveries stay in order"
        );
        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Accepted
        );
        let result = send_and_poll(sender, &mut self.pipeline, &packet.datagram);
        assert!(
            matches!(
                result,
                Ok(UdpPollOutcome::Processed {
                    source: observed,
                    admission: InsertOutcome::Inserted,
                }) if observed == source
            ),
            "fresh packet from {source} must be processed"
        );
        self.pinned = Some(source);
        self.assert_decoded_and_popped(sequence, payload, self.queue_epoch_dirty);
        self.queue_epoch_dirty = false;

        self.counters.authenticated += 1;
        self.next_expected = sequence.checked_add(1).expect("test sequences stay small");
        self.admitted_seqs.push(sequence);
        self.admitted_datagrams.push(packet.datagram);
    }

    /// Fills the fixed jitter buffer over the real UDP socket, then admits one
    /// more packet to force a controlled resynchronization. The resync packet
    /// must establish the new adaptive arrival baseline immediately while the
    /// authenticated source pin remains unchanged.
    fn deliver_resynchronized(&mut self) {
        self.step += 1;
        let source = self.source_a;
        assert_eq!(self.pinned, Some(source), "resync requires a bound source");
        let mut last_sequence = None;
        let mut last_payload = 0_u8;

        for index in 0..5 {
            let payload = u8::try_from((self.step + index) % 251).expect("payload stays below 251");
            let packet = self
                .sealer
                .seal(0, u64::from(self.next_expected) * 960, &[payload])
                .unwrap();
            let sequence = packet.header.sequence;
            assert_eq!(
                sequence, self.next_expected,
                "resync deliveries stay in order"
            );
            assert_eq!(
                self.replay.accept_authenticated(sequence),
                ReplayDecision::Accepted
            );
            let result = send_and_poll(&self.sender_a, &mut self.pipeline, &packet.datagram);
            let expected = if index == 4 {
                InsertOutcome::Resynchronized { dropped_frames: 4 }
            } else {
                InsertOutcome::Inserted
            };
            assert!(
                matches!(
                    result,
                    Ok(UdpPollOutcome::Processed {
                        source: observed,
                        admission,
                    }) if observed == source && admission == expected
                ),
                "resync packet must be admitted from the pinned source"
            );

            self.counters.authenticated += 1;
            self.next_expected = sequence.checked_add(1).expect("test sequences stay small");
            self.admitted_seqs.push(sequence);
            self.admitted_datagrams.push(packet.datagram);
            last_sequence = Some(sequence);
            last_payload = payload;
        }

        self.counters.jitter_resync_dropped_frames += 4;
        let snapshot = self.pipeline.counters().snapshot();
        assert_eq!(
            (
                snapshot.target_min_frames,
                snapshot.target_max_frames,
                snapshot.target_frames,
                snapshot.target_smoothed_jitter_ms,
                snapshot.target_raised_steps,
                snapshot.target_lowered_steps,
            ),
            (2, 4, 3, 0, 0, 0),
            "the UDP resync packet must publish a clean adaptive target epoch"
        );
        self.counters.stream_resets += 1;
        self.queue_epoch_dirty = true;
        self.assert_decoded_and_popped(
            last_sequence.expect("resync sequence exists"),
            last_payload,
            true,
        );
        self.queue_epoch_dirty = false;
    }

    /// Rotates to a strictly newer epoch; the pin survives rotation.
    fn rotate(&mut self) {
        self.step += 1;
        let next_epoch = self.epoch.checked_add(1).expect("test epochs stay small");
        let next_context = context(self.session_id, next_epoch);
        if let Some(last) = self.admitted_datagrams.last() {
            self.retired_epoch_packet = Some(last.clone());
        }
        self.pipeline.rotate(&self.secret, next_context, 0).unwrap();
        assert_eq!(
            self.pipeline.pinned_source(),
            self.pinned,
            "rotation keeps the pin"
        );
        self.epoch = next_epoch;
        self.sealer = MediaSealer::new(&self.secret, next_context).unwrap();
        self.replay = ReplayWindow::new();
        self.next_expected = 0;
        self.admitted_seqs.clear();
        self.admitted_datagrams.clear();
        self.queue_epoch_dirty = true;
        self.counters.stream_resets += 1;
    }

    /// Installs a fresh authenticated session; only this clears the pin.
    fn install_fresh_session(&mut self) {
        self.step += 1;
        let next_session = if self.session_id == SESSION_IDS[0] {
            SESSION_IDS[1]
        } else {
            SESSION_IDS[0]
        };
        let next_epoch = self.epoch.checked_add(1).expect("test epochs stay small");
        let next_context = context(next_session, next_epoch);
        if let Some(last) = self.admitted_datagrams.last() {
            self.retired_epoch_packet = Some(last.clone());
        }
        self.pipeline
            .install_fresh_session(&self.secret, next_context, 0)
            .unwrap();
        assert_eq!(
            self.pipeline.pinned_source(),
            None,
            "fresh session clears the pin"
        );
        self.session_id = next_session;
        self.epoch = next_epoch;
        self.sealer = MediaSealer::new(&self.secret, next_context).unwrap();
        self.pinned = None;
        self.replay = ReplayWindow::new();
        self.next_expected = 0;
        self.admitted_seqs.clear();
        self.admitted_datagrams.clear();
        self.queue_epoch_dirty = true;
        self.counters.stream_resets += 1;
    }

    /// Rejects oversized datagrams at the socket boundary before parser
    /// ingress.
    fn deliver_oversized(&mut self) {
        self.step += 1;
        let oversized = vec![0x5A; MAX_MEDIA_DATAGRAM_LEN + 1];
        let result = send_and_poll(&self.sender_a, &mut self.pipeline, &oversized);
        assert!(
            matches!(
                result,
                Err(UdpReceiverError::Socket(UdpReceiveError::DatagramTooLarge))
            ),
            "oversized datagram must be rejected"
        );
        self.counters.rejected += 1;
    }

    /// Delivers tampered ciphertext for a fresh unseen sequence, then its
    /// valid twin: failed AEAD must not advance the replay window or move the
    /// pin.
    fn deliver_tamper_and_twin(&mut self) {
        self.step += 1;
        let payload = u8::try_from(self.step % 251).expect("payload stays below 251");
        let packet = self
            .sealer
            .seal(0, u64::from(self.next_expected) * 960, &[payload])
            .unwrap();
        let sequence = packet.header.sequence;
        let mut tampered = packet.datagram.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x80;
        let result = send_and_poll(&self.sender_a, &mut self.pipeline, &tampered);
        assert!(
            matches!(
                result,
                Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                    IngressError::Authentication(MediaCryptoError::AuthenticationFailed)
                )))
            ),
            "tampered ciphertext must fail authentication"
        );
        self.counters.rejected += 1;

        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Accepted,
            "failed AEAD must not advance the replay window"
        );
        let result = send_and_poll(&self.sender_a, &mut self.pipeline, &packet.datagram);
        assert!(
            matches!(
                result,
                Ok(UdpPollOutcome::Processed {
                    admission: InsertOutcome::Inserted,
                    ..
                })
            ),
            "the valid twin of a failed packet must still be admitted"
        );
        self.pinned = Some(self.source_a);
        self.assert_decoded_and_popped(sequence, payload, self.queue_epoch_dirty);
        self.queue_epoch_dirty = false;

        self.counters.authenticated += 1;
        self.next_expected = sequence.checked_add(1).expect("test sequences stay small");
        self.admitted_seqs.push(sequence);
        self.admitted_datagrams.push(packet.datagram);
    }

    /// Replays an admitted datagram; the authenticated replay window rejects
    /// the duplicate.
    fn deliver_duplicate(&mut self, index: usize) {
        self.step += 1;
        let sequence = self.admitted_seqs[index];
        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Duplicate
        );
        let datagram = self.admitted_datagrams[index].clone();
        let result = send_and_poll(&self.sender_a, &mut self.pipeline, &datagram);
        assert!(
            matches!(
                result,
                Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                    IngressError::Replay(ReplayDecision::Duplicate)
                )))
            ),
            "duplicate delivery must be rejected by replay"
        );
        self.counters.authenticated += 1;
        self.counters.duplicate += 1;
        self.counters.rejected += 1;
    }

    /// Delivers a packet from a retired epoch; it is rejected as unknown.
    fn deliver_retired_epoch(&mut self) {
        self.step += 1;
        if let Some(old_datagram) = self.retired_epoch_packet.clone() {
            let result = send_and_poll(&self.sender_a, &mut self.pipeline, &old_datagram);
            assert!(
                matches!(
                    result,
                    Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                        IngressError::UnknownStream
                    )))
                ),
                "retired-epoch packets must be rejected as unknown"
            );
            self.counters.rejected += 1;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut offset = 0;
    let mut harness = UdpWalk::new(data);

    // Deterministic edge coverage on every input: oversized rejection, first
    // admission binding the pin, changed-endpoint rejection that never moves
    // the pin, a duplicate, tampered ciphertext with a valid-twin probe, a
    // rotation that keeps the pin, a retired-epoch rejection, a fresh
    // admission across the rotation, a fresh-session install that clears the
    // pin, and a fresh admission that rebinds it. The pipeline state is
    // asserted against the independent model after every operation.
    harness.deliver_oversized();
    harness.assert_state("edge: oversized");
    harness.deliver_fresh(false);
    harness.assert_state("edge: first admission");
    harness.deliver_resynchronized();
    harness.assert_state("edge: UDP resynchronization");
    harness.deliver_fresh(true);
    harness.assert_state("edge: changed endpoint");
    harness.deliver_duplicate(0);
    harness.assert_state("edge: duplicate");
    harness.deliver_tamper_and_twin();
    harness.assert_state("edge: tamper and twin");
    harness.rotate();
    harness.assert_state("edge: rotation");
    harness.deliver_retired_epoch();
    harness.assert_state("edge: retired epoch");
    harness.deliver_fresh(false);
    harness.assert_state("edge: fresh after rotation");
    harness.install_fresh_session();
    harness.assert_state("edge: fresh session");
    harness.deliver_fresh(false);
    harness.assert_state("edge: rebind");

    // Fuzz-derived deliveries and lifecycle transitions.
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        match next_u64(data, &mut offset) % 20 {
            0 => harness.rotate(),
            1 => harness.install_fresh_session(),
            2 => harness.deliver_oversized(),
            3 | 4 => harness.deliver_tamper_and_twin(),
            5 | 6 => {
                if !harness.admitted_seqs.is_empty() {
                    let index = next_u64(data, &mut offset) as usize % harness.admitted_seqs.len();
                    harness.deliver_duplicate(index);
                }
            }
            7 => harness.deliver_retired_epoch(),
            _ => {
                let from_b = harness.pinned == Some(harness.source_a)
                    && next_u64(data, &mut offset).is_multiple_of(3);
                harness.deliver_fresh(from_b);
            }
        }
        harness.assert_state("step");
    }

    assert!(
        harness.counters.authenticated > 0,
        "the mix must admit fresh packets"
    );
    assert!(
        harness.counters.duplicate > 0,
        "the mix must deliver duplicates"
    );
    assert!(harness.counters.rejected > 0, "the mix must reject packets");
    assert!(
        harness.counters.stream_resets > 0,
        "the mix must rotate or reinstall"
    );
});
