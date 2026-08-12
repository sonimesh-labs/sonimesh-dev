//! Deterministic property coverage for the UDP-backed protected receiver.
//!
//! `UdpReceiverPipeline` is the application-domain composition of the bounded
//! UDP socket, the fail-closed source pin, the protected ingress, and the
//! decode worker. The source pin must bind only to the endpoint of the first
//! successful protected outcome, a changed endpoint must be rejected before
//! any protected work and never move the pin, failed processing must never
//! bind an unbound source, and only a fresh authenticated session clears the
//! binding. This test drives randomized deliveries over real loopback sockets
//! against an independent pin + replay + counter model and asserts every poll
//! outcome, pin state, decoded PCM, and diagnostic counter.

use core::convert::Infallible;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::thread;

use sonimesh_app::{ReceiverPipelineError, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmConsumer, RealtimePcmPopOutcome,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{IngressError, UdpReceiveError};
use sonimesh_protocol::{
    MAX_MEDIA_DATAGRAM_LEN, MediaCryptoError, MediaDirection, MediaKeyContext, MediaSealer,
    ReplayDecision, ReplayWindow, SessionSecret,
};

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

const SESSION_IDS: [u64; 2] = [0x0102_0304_0506_0708, 0x0A0B_0C0D_0E0F_0102];

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

/// Decodes one admitted frame and pops the queued PCM block, asserting the
/// sequence, sample count, decoded payload byte, and queue discontinuity flag.
fn assert_decoded_and_popped(
    pipeline: &mut Pipeline,
    consumer: &mut RealtimePcmConsumer<4>,
    expected_sequence: u32,
    expected_payload: u8,
    expect_discontinuity: bool,
) {
    let decoded = pipeline.decode_next().unwrap();
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
    let popped = consumer.pop_into(&mut output);
    assert!(
        matches!(
            popped,
            RealtimePcmPopOutcome::Filled {
                samples: 1,
                sequence,
                discontinuity,
                discarded_stale_blocks: 0,
            } if sequence == u64::from(expected_sequence) && discontinuity == expect_discontinuity
        ),
        "expected a clean filled block for {expected_sequence}, got {popped:?}"
    );
    assert!(
        (output[0] - f32::from(expected_payload)).abs() < f32::EPSILON,
        "expected payload {expected_payload}, got {}",
        output[0]
    );
}

/// Seals, sends, and decodes one fresh packet from `sender`, updating the
/// independent pin, replay, and counter models. A changed endpoint is rejected
/// before protected work and never moves the pin.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn deliver_fresh(
    step: usize,
    sender: &UdpSocket,
    pipeline: &mut Pipeline,
    sealer: &mut MediaSealer,
    pinned: &mut Option<SocketAddr>,
    replay: &mut ReplayWindow,
    model: &mut CounterModel,
    next_expected: &mut u32,
    admitted_seqs: &mut Vec<u32>,
    admitted_datagrams: &mut Vec<Vec<u8>>,
    consumer: &mut RealtimePcmConsumer<4>,
    queue_epoch_dirty: &mut bool,
) {
    let source = sender.local_addr().unwrap();

    if pinned.is_some_and(|expected| expected != source) {
        // A different endpoint is rejected before parsing or authentication, so
        // the datagram content is irrelevant and the sealer must not advance.
        let result = send_and_poll(sender, pipeline, &[0x42]);
        assert!(
            matches!(result, Err(UdpReceiverError::SourceEndpointChanged)),
            "changed endpoint must be rejected"
        );
        model.rejected += 1;
        return;
    }

    let payload = u8::try_from(step % 251).expect("payload stays below 251");
    let packet = sealer
        .seal(0, u64::from(*next_expected) * 960, &[payload])
        .unwrap();
    let sequence = packet.header.sequence;

    assert_eq!(sequence, *next_expected, "fresh deliveries stay in order");
    assert_eq!(
        replay.accept_authenticated(sequence),
        ReplayDecision::Accepted
    );
    let result = send_and_poll(sender, pipeline, &packet.datagram);
    assert!(
        matches!(
            result,
            Ok(UdpPollOutcome::Processed { source: observed, admission: InsertOutcome::Inserted })
                if observed == source
        ),
        "fresh packet from {source} must be processed"
    );
    *pinned = Some(source);
    assert_decoded_and_popped(pipeline, consumer, sequence, payload, *queue_epoch_dirty);
    *queue_epoch_dirty = false;

    model.authenticated += 1;
    *next_expected = sequence.checked_add(1).expect("test sequences stay small");
    admitted_seqs.push(sequence);
    admitted_datagrams.push(packet.datagram);
}

/// Drives randomized deliveries through the real socket against the pin,
/// replay, and counter models, including rotations, fresh sessions, tampered
/// ciphertext with a valid-twin probe, duplicates, retired-epoch packets, and
/// oversized datagrams.
#[allow(clippy::too_many_lines)]
#[test]
fn randomized_udp_deliveries_match_an_independent_pin_replay_and_counter_model() {
    const STEPS: usize = 1_500;
    let mut rng = XorShift64(0xBEEF_0000_2026_0011);
    let secret = SessionSecret::new([0xA5; 32]);
    let mut session_id = SESSION_IDS[0];
    let mut epoch = 2_u16;
    let mut sealer = MediaSealer::new(&secret, context(session_id, epoch)).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context(session_id, epoch),
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let sender_a = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender_b = UdpSocket::bind("127.0.0.1:0").unwrap();
    let source_a = sender_a.local_addr().unwrap();

    // Independent model.
    let mut pinned: Option<SocketAddr> = None;
    let mut replay = ReplayWindow::new();
    let mut model = CounterModel::default();
    let mut next_expected = 0_u32;
    let mut admitted_seqs: Vec<u32> = Vec::new();
    let mut admitted_datagrams: Vec<Vec<u8>> = Vec::new();
    let mut retired_epoch_packet: Option<Vec<u8>> = None;
    let mut queue_epoch_dirty = false;

    for step in 0..STEPS {
        let roll = rng.next_u32() % 20;
        match roll {
            0 => {
                // Rotate to a strictly newer epoch; the pin survives rotation.
                let next_epoch = epoch.checked_add(1).expect("test epochs stay small");
                let next_context = context(session_id, next_epoch);
                if let Some(last) = admitted_datagrams.last() {
                    retired_epoch_packet = Some(last.clone());
                }
                pipeline.rotate(&secret, next_context, 0).unwrap();
                assert_eq!(pipeline.pinned_source(), pinned);
                epoch = next_epoch;
                sealer = MediaSealer::new(&secret, next_context).unwrap();
                replay = ReplayWindow::new();
                next_expected = 0;
                admitted_seqs.clear();
                admitted_datagrams.clear();
                queue_epoch_dirty = true;
                model.stream_resets += 1;
            }
            1 => {
                // A fresh authenticated session clears the source pin.
                let next_session = if session_id == SESSION_IDS[0] {
                    SESSION_IDS[1]
                } else {
                    SESSION_IDS[0]
                };
                let next_epoch = epoch.checked_add(1).expect("test epochs stay small");
                let next_context = context(next_session, next_epoch);
                if let Some(last) = admitted_datagrams.last() {
                    retired_epoch_packet = Some(last.clone());
                }
                pipeline
                    .install_fresh_session(&secret, next_context, 0)
                    .unwrap();
                assert_eq!(
                    pipeline.pinned_source(),
                    None,
                    "fresh session clears the pin"
                );
                session_id = next_session;
                epoch = next_epoch;
                sealer = MediaSealer::new(&secret, next_context).unwrap();
                pinned = None;
                replay = ReplayWindow::new();
                next_expected = 0;
                admitted_seqs.clear();
                admitted_datagrams.clear();
                queue_epoch_dirty = true;
                model.stream_resets += 1;
            }
            2 => {
                // Oversized datagrams are rejected by the socket boundary.
                let oversized = vec![0x5A; MAX_MEDIA_DATAGRAM_LEN + 1];
                let result = send_and_poll(&sender_a, &mut pipeline, &oversized);
                assert!(
                    matches!(
                        result,
                        Err(UdpReceiverError::Socket(UdpReceiveError::DatagramTooLarge))
                    ),
                    "oversized datagram must be rejected"
                );
                model.rejected += 1;
            }
            3 | 4 => {
                // Tamper a fresh unseen sequence, then deliver its valid twin:
                // failed AEAD must not advance the replay window or move the pin.
                let payload = u8::try_from(step % 251).expect("payload stays below 251");
                let packet = sealer
                    .seal(0, u64::from(next_expected) * 960, &[payload])
                    .unwrap();
                let sequence = packet.header.sequence;
                let mut tampered = packet.datagram.clone();
                let last = tampered.len() - 1;
                tampered[last] ^= 0x80;
                let result = send_and_poll(&sender_a, &mut pipeline, &tampered);
                assert!(
                    matches!(
                        result,
                        Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                            IngressError::Authentication(MediaCryptoError::AuthenticationFailed)
                        )))
                    ),
                    "tampered ciphertext must fail authentication"
                );
                model.rejected += 1;

                assert_eq!(
                    replay.accept_authenticated(sequence),
                    ReplayDecision::Accepted,
                    "failed AEAD must not advance the replay window"
                );
                let result = send_and_poll(&sender_a, &mut pipeline, &packet.datagram);
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
                pinned = Some(source_a);
                assert_decoded_and_popped(
                    &mut pipeline,
                    &mut consumer,
                    sequence,
                    payload,
                    queue_epoch_dirty,
                );
                queue_epoch_dirty = false;

                model.authenticated += 1;
                next_expected = sequence.checked_add(1).expect("test sequences stay small");
                admitted_seqs.push(sequence);
                admitted_datagrams.push(packet.datagram);
            }
            5 | 6 => {
                // Duplicates are rejected by the authenticated replay window.
                if let Some(index) = admitted_seqs
                    .len()
                    .checked_sub(1)
                    .map(|last| rng.next_usize(last + 1))
                {
                    let sequence = admitted_seqs[index];
                    assert_eq!(
                        replay.accept_authenticated(sequence),
                        ReplayDecision::Duplicate
                    );
                    let result =
                        send_and_poll(&sender_a, &mut pipeline, &admitted_datagrams[index]);
                    assert!(
                        matches!(
                            result,
                            Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                                IngressError::Replay(ReplayDecision::Duplicate)
                            )))
                        ),
                        "duplicate delivery must be rejected by replay"
                    );
                    model.authenticated += 1;
                    model.duplicate += 1;
                    model.rejected += 1;
                }
            }
            7 => {
                // Packets from a retired epoch are rejected as unknown.
                if let Some(old_datagram) = &retired_epoch_packet {
                    let result = send_and_poll(&sender_a, &mut pipeline, old_datagram);
                    assert!(
                        matches!(
                            result,
                            Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                                IngressError::UnknownStream
                            )))
                        ),
                        "retired-epoch packets must be rejected as unknown"
                    );
                    model.rejected += 1;
                }
            }
            _ => {
                // Fresh in-order packet, mostly from the pinned sender but
                // sometimes from a second endpoint to exercise the pin.
                let sender = if pinned.is_some_and(|expected| expected == source_a)
                    && rng.next_u32() % 3 == 0
                {
                    &sender_b
                } else {
                    &sender_a
                };
                deliver_fresh(
                    step,
                    sender,
                    &mut pipeline,
                    &mut sealer,
                    &mut pinned,
                    &mut replay,
                    &mut model,
                    &mut next_expected,
                    &mut admitted_seqs,
                    &mut admitted_datagrams,
                    &mut consumer,
                    &mut queue_epoch_dirty,
                );
            }
        }

        assert!(
            matches!(pipeline.poll().unwrap(), UdpPollOutcome::Empty),
            "every delivered datagram must be consumed by the previous arm"
        );
        assert_eq!(
            pipeline.pinned_source(),
            pinned,
            "pinned source model drift"
        );
        assert_eq!(
            consumer.active_len(),
            0,
            "decode plus pop keeps the PCM queue empty"
        );
        let snapshot = pipeline.counters().snapshot();
        assert_eq!(
            snapshot.authenticated_packets, model.authenticated,
            "authenticated counter drift"
        );
        assert_eq!(
            snapshot.duplicate_packets, model.duplicate,
            "duplicate counter drift"
        );
        assert_eq!(
            snapshot.rejected_packets, model.rejected,
            "rejected counter drift"
        );
        assert_eq!(
            snapshot.stream_resets, model.stream_resets,
            "stream reset counter drift"
        );
    }

    assert!(model.authenticated > 0, "the mix must admit fresh packets");
    assert!(model.duplicate > 0, "the mix must deliver duplicates");
    assert!(model.rejected > 0, "the mix must reject packets");
    assert!(model.stream_resets > 0, "the mix must rotate or reinstall");
}
