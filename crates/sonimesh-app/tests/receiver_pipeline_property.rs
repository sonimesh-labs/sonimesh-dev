//! Deterministic property coverage for the protected receiver pipeline.
//!
//! `ReceiverStreamPipeline` is the non-realtime composition of one protected
//! ingress and one decode worker: only authenticated in-window packets may
//! reach the decoder, failed AEAD must never advance the replay window, failed
//! rotations must not touch installed context or playout state, and the
//! published counters must agree with an independent model of every delivery.
//! This test drives randomized in-order deliveries, duplicates, tampered
//! ciphertext with a valid-twin probe, retired-epoch packets, and rotations
//! against a `ReplayWindow` oracle plus a simple counter model, asserting
//! every outcome, plaintext zeroing, decoded PCM, and counter equality.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{ReceiverPipelineError, ReceiverStreamPipeline};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmConsumer, RealtimePcmPopOutcome,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{IngressError, ProtectedIngressError};
use sonimesh_protocol::{
    MediaCryptoError, MediaDirection, MediaKeyContext, MediaSealer, ReplayDecision, ReplayWindow,
    SessionSecret,
};

type Pipeline = ReceiverStreamPipeline<ByteDecoder, 4, 8, 4>;

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

fn context(epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        epoch,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

/// Independent cumulative counter model for the pipeline's diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CounterModel {
    authenticated: u64,
    duplicate: u64,
    rejected: u64,
    stream_resets: u64,
}

/// Decodes one admitted frame and pops the queued PCM block, asserting the
/// sequence, sample count, and decoded payload byte.
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

/// Seals, delivers, and decodes one fresh in-order packet, then updates the
/// independent model and the admitted-packet history.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn deliver_fresh(
    step: usize,
    pipeline: &mut Pipeline,
    sealer: &mut MediaSealer,
    replay: &mut ReplayWindow,
    model: &mut CounterModel,
    next_expected: &mut u32,
    admitted_seqs: &mut Vec<u32>,
    admitted_datagrams: &mut Vec<Vec<u8>>,
    consumer: &mut RealtimePcmConsumer<4>,
    plaintext: &mut [u8; 8],
    queue_epoch_dirty: &mut bool,
) {
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

    let result = pipeline.receive(&packet.datagram, plaintext);
    assert!(
        matches!(result, Ok(InsertOutcome::Inserted)),
        "fresh packet must be inserted"
    );
    assert!(
        plaintext.iter().all(|byte| *byte == 0),
        "plaintext must be cleared after admission"
    );

    assert_decoded_and_popped(pipeline, consumer, sequence, payload, *queue_epoch_dirty);
    *queue_epoch_dirty = false;

    model.authenticated += 1;
    *next_expected = sequence.checked_add(1).expect("test sequences stay small");
    admitted_seqs.push(sequence);
    admitted_datagrams.push(packet.datagram);
}

/// Drives randomized deliveries through the pipeline against the replay oracle
/// and counter model, including rotations that retire the previous epoch.
#[allow(clippy::too_many_lines)]
#[test]
fn randomized_deliveries_match_an_independent_replay_and_counter_model() {
    const STEPS: usize = 2_000;
    let mut rng = XorShift64(0x51A7_CCE5_2026_0010);
    let secret = SessionSecret::new([0xA5; 32]);
    let mut epoch = 2_u16;
    let mut sealer = MediaSealer::new(&secret, context(epoch)).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = Pipeline::new(
        &secret,
        context(epoch),
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();

    // Independent model.
    let mut replay = ReplayWindow::new();
    let mut model = CounterModel::default();
    let mut next_expected = 0_u32;
    let mut admitted_seqs: Vec<u32> = Vec::new();
    let mut admitted_datagrams: Vec<Vec<u8>> = Vec::new();
    let mut retired_epoch_packet: Option<Vec<u8>> = None;
    let mut queue_epoch_dirty = false;
    let mut plaintext = [0_u8; 8];

    for step in 0..STEPS {
        let roll = rng.next_u32() % 100;
        if roll < 4 {
            let next_epoch = epoch.checked_add(1).expect("test epochs stay small");
            let next_context = context(next_epoch);
            if let Some(last) = admitted_datagrams.last() {
                retired_epoch_packet = Some(last.clone());
            }
            pipeline.rotate(&secret, next_context, 0).unwrap();
            assert_eq!(pipeline.context(), next_context);
            epoch = next_epoch;
            sealer = MediaSealer::new(&secret, next_context).unwrap();
            replay = ReplayWindow::new();
            next_expected = 0;
            admitted_seqs.clear();
            admitted_datagrams.clear();
            queue_epoch_dirty = true;
            model.stream_resets += 1;
            continue;
        }

        let arm = roll % 4;
        match arm {
            0 => deliver_fresh(
                step,
                &mut pipeline,
                &mut sealer,
                &mut replay,
                &mut model,
                &mut next_expected,
                &mut admitted_seqs,
                &mut admitted_datagrams,
                &mut consumer,
                &mut plaintext,
                &mut queue_epoch_dirty,
            ),
            1 => {
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
                    let result = pipeline.receive(&admitted_datagrams[index], &mut plaintext);
                    assert!(
                        matches!(
                            result,
                            Err(ReceiverPipelineError::Ingress(IngressError::Replay(
                                ReplayDecision::Duplicate
                            )))
                        ),
                        "duplicate delivery must be rejected by replay"
                    );
                    assert!(
                        plaintext.iter().all(|byte| *byte == 0),
                        "rejected duplicate must zero plaintext"
                    );
                    model.authenticated += 1;
                    model.duplicate += 1;
                    model.rejected += 1;
                }
            }
            2 => {
                // Tamper a fresh unseen sequence, then deliver its valid twin:
                // the failed AEAD must not advance the replay window.
                let payload = u8::try_from(step % 251).expect("payload stays below 251");
                let packet = sealer
                    .seal(0, u64::from(next_expected) * 960, &[payload])
                    .unwrap();
                let sequence = packet.header.sequence;

                let mut tampered = packet.datagram.clone();
                let last = tampered.len() - 1;
                tampered[last] ^= 0x80;
                let result = pipeline.receive(&tampered, &mut plaintext);
                assert!(
                    matches!(
                        result,
                        Err(ReceiverPipelineError::Ingress(
                            IngressError::Authentication(MediaCryptoError::AuthenticationFailed)
                        ))
                    ),
                    "tampered ciphertext must fail authentication"
                );
                assert!(
                    plaintext.iter().all(|byte| *byte == 0),
                    "failed AEAD must zero plaintext"
                );
                model.rejected += 1;

                assert_eq!(
                    replay.accept_authenticated(sequence),
                    ReplayDecision::Accepted,
                    "failed AEAD must not advance the replay window"
                );
                let result = pipeline.receive(&packet.datagram, &mut plaintext);
                assert!(
                    matches!(result, Ok(InsertOutcome::Inserted)),
                    "the valid twin of a failed packet must still be admitted"
                );
                assert!(
                    plaintext.iter().all(|byte| *byte == 0),
                    "plaintext must be cleared after admission"
                );
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
            3 => {
                if let Some(old_datagram) = &retired_epoch_packet {
                    let result = pipeline.receive(old_datagram, &mut plaintext);
                    assert!(
                        matches!(
                            result,
                            Err(ReceiverPipelineError::Ingress(IngressError::UnknownStream))
                        ),
                        "retired-epoch packets must be rejected as unknown"
                    );
                    assert!(
                        plaintext.iter().all(|byte| *byte == 0),
                        "rejected retired packet must zero plaintext"
                    );
                    model.rejected += 1;
                }
            }
            _ => unreachable!("arm is bounded by modulo 4"),
        }

        assert_eq!(
            pipeline.queued_pcm_blocks(),
            0,
            "decode plus pop keeps the PCM queue empty"
        );
        let snapshot = counters.snapshot();
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
    assert!(model.stream_resets > 0, "the mix must rotate epochs");
}

/// Failed rotations must leave the installed context, replay state, decode
/// state, and cumulative counters untouched.
#[test]
fn failed_rotation_leaves_context_replay_and_counters_unchanged() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let mut sealer = MediaSealer::new(&secret, initial).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = Pipeline::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    // Admit one packet so replay and decode state exist before the attempts.
    let first = sealer.seal(0, 0, &[7]).unwrap();
    assert!(matches!(
        pipeline.receive(&first.datagram, &mut plaintext),
        Ok(InsertOutcome::Inserted)
    ));
    assert_decoded_and_popped(&mut pipeline, &mut consumer, 0, 7, false);

    let same_epoch = context(2);
    assert!(matches!(
        pipeline.rotate(&secret, same_epoch, 0),
        Err(ProtectedIngressError::NonIncreasingEpoch)
    ));
    assert_eq!(pipeline.context(), initial);

    let lower_epoch = context(1);
    assert!(matches!(
        pipeline.rotate(&secret, lower_epoch, 0),
        Err(ProtectedIngressError::NonIncreasingEpoch)
    ));
    assert_eq!(pipeline.context(), initial);

    let changed_stream = MediaKeyContext::new(
        initial.session_id,
        initial.stream_id + 1,
        3,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap();
    assert!(matches!(
        pipeline.rotate(&secret, changed_stream, 0),
        Err(ProtectedIngressError::RotationContextMismatch)
    ));
    assert_eq!(pipeline.context(), initial);

    // Replay and decode state are intact: the next in-order packet still
    // admits and decodes, and no failed rotation reset stream state.
    let second = sealer.seal(0, 960, &[8]).unwrap();
    assert!(matches!(
        pipeline.receive(&second.datagram, &mut plaintext),
        Ok(InsertOutcome::Inserted)
    ));
    assert_decoded_and_popped(&mut pipeline, &mut consumer, 1, 8, false);

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.stream_resets, 0, "failed rotations must not reset");
    assert_eq!(snapshot.authenticated_packets, 2);
    assert_eq!(snapshot.rejected_packets, 0);
}
