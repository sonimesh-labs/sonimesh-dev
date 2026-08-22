#![no_main]

//! Fuzz the protected receiver pipeline composition.
//!
//! `ReceiverStreamPipeline` is the non-realtime composition of one protected
//! ingress and one decode worker: only authenticated in-window packets may
//! reach the decoder, failed AEAD must never advance the replay window,
//! retired-epoch packets are rejected as unknown, and the published counters
//! must agree with an independent model of every delivery. This target drives
//! byte-selected in-order deliveries, duplicates, tampered ciphertext with a
//! valid-twin probe, retired-epoch packets, rotations, and pending-signal
//! gated playouts against a `ReplayWindow` oracle plus simple counter and
//! gate models, asserting every outcome, plaintext zeroing, decoded PCM, and
//! counter equality after each step.
//!
//! The model mirrors the seeded property walks
//! (`crates/sonimesh-app/tests/receiver_pipeline_property.rs` and
//! `crates/sonimesh-app/tests/receiver_pipeline_target_gated_playout.rs`) so
//! the deterministic and byte-driven paths assert the same contract.

use core::convert::Infallible;
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{AdaptiveTargetConfig, ReceiverPipelineError, ReceiverStreamPipeline};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, JitterMode, JitterTargetConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, TargetGatedPlayoutConfig, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::IngressError;
use sonimesh_protocol::{
    MediaCryptoError, MediaDirection, MediaKeyContext, MediaSealer, ReplayDecision, ReplayWindow,
    SessionSecret,
};

type Pipeline = ReceiverStreamPipeline<ByteDecoder, 4, 8, 4>;

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

fn context(epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        epoch,
        MediaDirection::AndroidToDesktop,
    )
    .expect("fixed test context is valid")
}

/// Independent cumulative counter model for the pipeline's diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CounterModel {
    authenticated: u64,
    duplicate: u64,
    rejected: u64,
    jitter_resync_dropped_frames: u64,
    stream_resets: u64,
    /// Per-epoch scheduler telemetry mirrored against the published snapshot.
    gated_ticks: u64,
    forced_playouts: u64,
    /// Scheduler telemetry accumulated across every stream epoch. Stream
    /// retirements reset the published per-epoch counters, so these totals
    /// are the only basis for asserting that a run exercised gating at all.
    total_gated_ticks: u64,
    total_forced_playouts: u64,
}

impl CounterModel {
    /// Folds one pending-arrival below-target tick into the gate model.
    fn gate_pending_below_target(&mut self) {
        self.gated_ticks = self.gated_ticks.saturating_add(1);
        self.total_gated_ticks = self.total_gated_ticks.saturating_add(1);
    }

    /// Folds one deadline-forced below-target playout into the gate model.
    fn force_forced_playout(&mut self) {
        self.forced_playouts = self.forced_playouts.saturating_add(1);
        self.total_forced_playouts = self.total_forced_playouts.saturating_add(1);
    }

    // A drained-arrival play (`arrivals_pending = false`) resets the streak
    // without touching either cumulative counter, so the drain-path decode
    // helper folds nothing into this model.

    /// Clears the scheduler telemetry for a fresh stream epoch.
    fn reset_gate_counters(&mut self) {
        self.gated_ticks = 0;
        self.forced_playouts = 0;
    }
}

/// Decodes one admitted frame through the gated drain path and pops the
/// queued PCM block, asserting the producer-local sequence, sample count, and
/// decoded payload byte.
///
/// `expected_pcm_sequence` is the model's producer-local block counter, not
/// the packet sequence: every stream-local retirement (`begin_epoch`) resets
/// that counter to zero inside a fresh queue epoch.
fn assert_decoded_and_popped(
    pipeline: &mut Pipeline,
    consumer: &mut RealtimePcmConsumer<4>,
    expected_sequence: u32,
    expected_payload: u8,
    expected_pcm_sequence: u64,
    expect_discontinuity: bool,
) {
    let decoded = pipeline.decode_next_with_arrivals_pending(false).unwrap();
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
            } if sequence == expected_pcm_sequence && discontinuity == expect_discontinuity
        ),
        "expected a clean filled block at producer-local sequence {expected_pcm_sequence}, \
         got {popped:?}"
    );
    assert!(
        (output[0] - f32::from(expected_payload)).abs() < f32::EPSILON,
        "expected payload {expected_payload}, got {}",
        output[0]
    );
}

/// Real protected pipeline plus its replay oracle and counter model.
struct PipelineHarness {
    pipeline: Pipeline,
    sealer: MediaSealer,
    replay: ReplayWindow,
    model: CounterModel,
    next_expected: u32,
    admitted_seqs: Vec<u32>,
    admitted_datagrams: Vec<Vec<u8>>,
    retired_epoch_packet: Option<Vec<u8>>,
    consumer: RealtimePcmConsumer<4>,
    plaintext: [u8; 8],
    queue_epoch_dirty: bool,
    /// Model of the PCM producer's local block counter. Every stream-local
    /// retirement (`begin_epoch`) resets it to zero inside a new queue epoch.
    next_pcm_sequence: u64,
    counters: Arc<StreamCounters>,
    secret: SessionSecret,
    epoch: u16,
}

impl PipelineHarness {
    fn new(data: &[u8]) -> Self {
        let secret = SessionSecret::new([0xA5; 32]);
        let epoch = 2_u16;
        let context = context(epoch);
        let sealer = MediaSealer::new(&secret, context).expect("fixed sealer is valid");
        let (producer, consumer) =
            realtime_pcm_queue::<4>(4).expect("fixed queue capacity is valid");
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
        let mut pipeline = Pipeline::new_with_adaptive_target(
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
        .expect("fixed adaptive pipeline setup is valid");
        pipeline
            .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
            .expect("the fixed gate deadline is valid");
        Self {
            pipeline,
            sealer,
            replay: ReplayWindow::new(),
            model: CounterModel::default(),
            next_expected: 0,
            admitted_seqs: Vec::new(),
            admitted_datagrams: Vec::new(),
            retired_epoch_packet: None,
            consumer,
            plaintext: [0_u8; 8],
            queue_epoch_dirty: false,
            next_pcm_sequence: 0,
            counters,
            secret,
            epoch,
        }
    }

    fn assert_state(&self) {
        assert_eq!(
            self.pipeline.queued_pcm_blocks(),
            0,
            "decode plus pop keeps the PCM queue empty"
        );
        let snapshot = self.counters.snapshot();
        assert_eq!(
            snapshot.authenticated_packets, self.model.authenticated,
            "authenticated counter drift"
        );
        assert_eq!(
            snapshot.duplicate_packets, self.model.duplicate,
            "duplicate counter drift"
        );
        assert_eq!(
            snapshot.rejected_packets, self.model.rejected,
            "rejected counter drift"
        );
        assert_eq!(
            snapshot.jitter_resync_dropped_frames, self.model.jitter_resync_dropped_frames,
            "jitter resynchronization drop counter drift"
        );
        assert_eq!(
            snapshot.stream_resets, self.model.stream_resets,
            "stream reset counter drift"
        );
        if snapshot.target_min_frames == 0 {
            assert_eq!(
                (snapshot.target_max_frames, snapshot.target_frames),
                (0, 0),
                "a retired target epoch must publish an empty state"
            );
            assert_eq!(
                (
                    snapshot.target_smoothed_jitter_ms,
                    snapshot.target_raised_steps
                ),
                (0, 0),
                "a retired target epoch must clear jitter and upward steps"
            );
            assert_eq!(
                snapshot.target_lowered_steps, 0,
                "a retired target epoch must clear downward steps"
            );
        } else {
            assert_eq!(
                (snapshot.target_min_frames, snapshot.target_max_frames),
                (2, 4),
                "the balanced target preset must remain bounded"
            );
            assert!(
                (2..=4).contains(&snapshot.target_frames),
                "the active target must stay within its water levels"
            );
            assert!(
                u64::from(snapshot.target_raised_steps) + u64::from(snapshot.target_lowered_steps)
                    <= self.model.authenticated,
                "target step counters must not exceed authenticated observations"
            );
        }
        assert_eq!(
            (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
            (self.model.gated_ticks, self.model.forced_playouts),
            "target-gated scheduler counter drift"
        );
    }

    /// Seals, delivers, and decodes one fresh in-order packet, then updates the
    /// independent model and the admitted-packet history.
    fn deliver_fresh(&mut self, step: usize) {
        let payload = u8::try_from(step % 251).expect("payload stays below 251");
        let packet = self
            .sealer
            .seal(0, u64::from(self.next_expected) * 960, &[payload])
            .expect("sealing a fresh packet cannot fail");
        let sequence = packet.header.sequence;
        assert_eq!(
            sequence, self.next_expected,
            "fresh deliveries stay in order"
        );
        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Accepted
        );

        let result = self.pipeline.receive(&packet.datagram, &mut self.plaintext);
        assert!(
            matches!(result, Ok(InsertOutcome::Inserted)),
            "fresh packet must be inserted"
        );
        assert!(
            self.plaintext.iter().all(|byte| *byte == 0),
            "plaintext must be cleared after admission"
        );

        assert_decoded_and_popped(
            &mut self.pipeline,
            &mut self.consumer,
            sequence,
            payload,
            self.next_pcm_sequence,
            self.queue_epoch_dirty,
        );
        self.queue_epoch_dirty = false;
        let pcm_sequence = self.next_pcm_sequence;
        self.next_pcm_sequence = pcm_sequence
            .checked_add(1)
            .expect("fuzz PCM sequences stay small");

        self.model.authenticated += 1;
        self.next_expected = sequence.checked_add(1).expect("fuzz sequences stay small");
        self.admitted_seqs.push(sequence);
        self.admitted_datagrams.push(packet.datagram);
    }

    /// Admits one fresh packet, then drives pending-arrival ticks until the
    /// fixed gate deadline forces the below-target playout of that frame.
    /// The deadline must always complete within three ticks because the
    /// balanced preset targets more than one buffered frame.
    fn deliver_gated(&mut self, step: usize) {
        let payload = u8::try_from(step % 251).expect("payload stays below 251");
        let packet = self
            .sealer
            .seal(0, u64::from(self.next_expected) * 960, &[payload])
            .expect("sealing a fresh packet cannot fail");
        let sequence = packet.header.sequence;
        assert_eq!(
            sequence, self.next_expected,
            "gated deliveries stay in order"
        );
        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Accepted
        );
        let result = self.pipeline.receive(&packet.datagram, &mut self.plaintext);
        assert!(
            matches!(result, Ok(InsertOutcome::Inserted)),
            "the gated packet must be inserted"
        );
        assert!(
            self.plaintext.iter().all(|byte| *byte == 0),
            "plaintext must be cleared after admission"
        );
        self.model.authenticated += 1;

        for _ in 0..2 {
            assert_eq!(
                self.pipeline
                    .decode_next_with_arrivals_pending(true)
                    .unwrap(),
                DecodeStep::NotReady,
                "a below-target pending tick must gate"
            );
            self.model.gate_pending_below_target();
        }
        let forced = self
            .pipeline
            .decode_next_with_arrivals_pending(true)
            .unwrap();
        assert!(
            matches!(forced, DecodeStep::Decoded { .. }),
            "the expired deadline must force the playout, got {forced:?}"
        );
        self.model.force_forced_playout();

        let mut output = [f32::NAN; 4];
        let popped = self.consumer.pop_into(&mut output);
        assert!(
            matches!(
                popped,
                RealtimePcmPopOutcome::Filled {
                    samples: 1,
                    sequence: observed,
                    discontinuity,
                    discarded_stale_blocks: 0,
                } if observed == self.next_pcm_sequence
                    && discontinuity == self.queue_epoch_dirty
            ),
            "expected a clean filled block at producer-local sequence {}, got {popped:?}",
            self.next_pcm_sequence
        );
        self.queue_epoch_dirty = false;
        let pcm_sequence = self.next_pcm_sequence;
        self.next_pcm_sequence = pcm_sequence
            .checked_add(1)
            .expect("fuzz PCM sequences stay small");

        self.next_expected = sequence.checked_add(1).expect("fuzz sequences stay small");
        self.admitted_seqs.push(sequence);
        self.admitted_datagrams.push(packet.datagram);
    }

    /// Fills the fixed jitter buffer without decoding, then admits one more
    /// packet to force a controlled resynchronization. The resync packet must
    /// establish the new adaptive arrival baseline immediately.
    fn deliver_resynchronized(&mut self, step: usize) {
        let mut last_sequence = None;
        let mut last_payload = 0_u8;
        for index in 0..5 {
            let payload = u8::try_from((step + index) % 251).expect("payload stays below 251");
            let packet = self
                .sealer
                .seal(0, u64::from(self.next_expected) * 960, &[payload])
                .expect("sealing a resync packet cannot fail");
            let sequence = packet.header.sequence;
            assert_eq!(
                sequence, self.next_expected,
                "resync deliveries stay in order"
            );
            assert_eq!(
                self.replay.accept_authenticated(sequence),
                ReplayDecision::Accepted
            );
            let result = self.pipeline.receive(&packet.datagram, &mut self.plaintext);
            let expected = if index == 4 {
                InsertOutcome::Resynchronized { dropped_frames: 4 }
            } else {
                InsertOutcome::Inserted
            };
            assert_eq!(result.unwrap(), expected, "resync admission outcome");
            assert!(
                self.plaintext.iter().all(|byte| *byte == 0),
                "plaintext must be cleared after resync admission"
            );

            self.model.authenticated += 1;
            self.next_expected = sequence.checked_add(1).expect("fuzz sequences stay small");
            self.admitted_seqs.push(sequence);
            self.admitted_datagrams.push(packet.datagram);
            last_sequence = Some(sequence);
            last_payload = payload;
        }

        self.model.jitter_resync_dropped_frames += 4;
        let snapshot = self.counters.snapshot();
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
            "the resync packet must publish a clean adaptive target epoch"
        );
        self.model.stream_resets += 1;
        self.queue_epoch_dirty = true;
        // The jitter resynchronization retires the PCM producer's queue epoch,
        // so the next block is producer-local sequence 0 and always
        // discontinuous regardless of the pre-resync model state.
        assert_decoded_and_popped(
            &mut self.pipeline,
            &mut self.consumer,
            last_sequence.expect("resync sequence exists"),
            last_payload,
            0,
            true,
        );
        self.queue_epoch_dirty = false;
        self.next_pcm_sequence = 1;
    }

    /// Delivers an admitted packet again; the replay window must reject it as
    /// a duplicate or as out-of-window stale, and the plaintext must be
    /// cleared either way.
    fn deliver_duplicate(&mut self, data: &[u8], offset: &mut usize) {
        let Some(last) = self.admitted_seqs.len().checked_sub(1) else {
            return;
        };
        let index = usize::try_from(next_u64(data, offset) % u64::try_from(last + 1).unwrap())
            .expect("index stays within usize");
        let sequence = self.admitted_seqs[index];
        // Re-recording an already-admitted sequence never mutates the oracle,
        // so this query predicts the exact decision without advancing the
        // stream's replay state.
        let decision = self.replay.accept_authenticated(sequence);
        assert!(
            matches!(decision, ReplayDecision::Duplicate | ReplayDecision::TooOld),
            "an admitted sequence must replay as duplicate or stale, got {decision:?}"
        );
        let result = self
            .pipeline
            .receive(&self.admitted_datagrams[index], &mut self.plaintext);
        assert!(
            matches!(
                result,
                Err(ReceiverPipelineError::Ingress(IngressError::Replay(observed)))
                    if observed == decision
            ),
            "replaying an admitted packet must be rejected as {decision:?}, got {result:?}"
        );
        assert!(
            self.plaintext.iter().all(|byte| *byte == 0),
            "rejected duplicate must zero plaintext"
        );
        self.model.authenticated += 1;
        if decision == ReplayDecision::Duplicate {
            self.model.duplicate += 1;
        }
        self.model.rejected += 1;
    }

    /// Tamper a fresh unseen sequence, then deliver its valid twin: the failed
    /// AEAD must not advance the replay window.
    fn deliver_tamper_twin(&mut self, step: usize) {
        let payload = u8::try_from(step % 251).expect("payload stays below 251");
        let packet = self
            .sealer
            .seal(0, u64::from(self.next_expected) * 960, &[payload])
            .expect("sealing a fresh packet cannot fail");
        let sequence = packet.header.sequence;

        let mut tampered = packet.datagram.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x80;
        let result = self.pipeline.receive(&tampered, &mut self.plaintext);
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
            self.plaintext.iter().all(|byte| *byte == 0),
            "failed AEAD must zero plaintext"
        );
        self.model.rejected += 1;

        assert_eq!(
            self.replay.accept_authenticated(sequence),
            ReplayDecision::Accepted,
            "failed AEAD must not advance the replay window"
        );
        let result = self.pipeline.receive(&packet.datagram, &mut self.plaintext);
        assert!(
            matches!(result, Ok(InsertOutcome::Inserted)),
            "the valid twin of a failed packet must still be admitted"
        );
        assert!(
            self.plaintext.iter().all(|byte| *byte == 0),
            "plaintext must be cleared after admission"
        );
        assert_decoded_and_popped(
            &mut self.pipeline,
            &mut self.consumer,
            sequence,
            payload,
            self.next_pcm_sequence,
            self.queue_epoch_dirty,
        );
        self.queue_epoch_dirty = false;
        let pcm_sequence = self.next_pcm_sequence;
        self.next_pcm_sequence = pcm_sequence
            .checked_add(1)
            .expect("fuzz PCM sequences stay small");

        self.model.authenticated += 1;
        self.next_expected = sequence.checked_add(1).expect("fuzz sequences stay small");
        self.admitted_seqs.push(sequence);
        self.admitted_datagrams.push(packet.datagram);
    }

    /// Replays the last packet of the retired epoch; it must be rejected as
    /// unknown and the plaintext cleared.
    fn deliver_retired(&mut self) {
        let Some(old_datagram) = &self.retired_epoch_packet else {
            return;
        };
        let result = self.pipeline.receive(old_datagram, &mut self.plaintext);
        assert!(
            matches!(
                result,
                Err(ReceiverPipelineError::Ingress(IngressError::UnknownStream))
            ),
            "retired-epoch packets must be rejected as unknown"
        );
        assert!(
            self.plaintext.iter().all(|byte| *byte == 0),
            "rejected retired packet must zero plaintext"
        );
        self.model.rejected += 1;
    }

    /// Rotates to the next epoch: replay, sealer, and admitted history reset,
    /// and the last admitted datagram becomes the retired-epoch probe.
    fn rotate(&mut self) {
        let next_epoch = self.epoch.checked_add(1).expect("fuzz epochs stay small");
        let next_context = context(next_epoch);
        if let Some(last) = self.admitted_datagrams.last() {
            self.retired_epoch_packet = Some(last.clone());
        }
        self.pipeline
            .rotate(&self.secret, next_context, 0)
            .expect("rotation to a higher epoch cannot fail");
        assert_eq!(self.pipeline.context(), next_context);
        self.epoch = next_epoch;
        self.sealer =
            MediaSealer::new(&self.secret, next_context).expect("rotation sealer is valid");
        self.replay = ReplayWindow::new();
        self.next_expected = 0;
        self.admitted_seqs.clear();
        self.admitted_datagrams.clear();
        self.queue_epoch_dirty = true;
        // The rotation retires the PCM producer's queue epoch, so its local
        // block counter restarts at zero and the first new-epoch block pops as
        // discontinuous.
        self.next_pcm_sequence = 0;
        self.model.stream_resets += 1;
        self.model.reset_gate_counters();
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut harness = PipelineHarness::new(data);
    let mut offset = 0;
    // Force one bounded jitter resynchronization on every input so the fuzz
    // target covers the adaptive epoch reset and immediate baseline contract,
    // then one deadline-forced gated playout so the scheduler counters and
    // streak model are exercised deterministically.
    harness.deliver_fresh(0);
    harness.assert_state();
    harness.deliver_resynchronized(1);
    harness.assert_state();
    harness.deliver_gated(2);
    harness.assert_state();
    let steps = 1 + data.len() % 128;
    for step in 0..steps {
        let roll = next_byte(data, &mut offset) % 100;
        if roll < 4 {
            harness.rotate();
        } else {
            match next_byte(data, &mut offset) % 5 {
                0 => harness.deliver_fresh(step),
                1 => harness.deliver_duplicate(data, &mut offset),
                2 => harness.deliver_tamper_twin(step),
                3 => harness.deliver_gated(step),
                _ => harness.deliver_retired(),
            }
        }
        harness.assert_state();
    }

    assert!(
        harness.model.total_gated_ticks > 0,
        "the mix must gate below-target pending ticks"
    );
    assert!(
        harness.model.total_forced_playouts > 0,
        "the mix must complete gated playouts through the deadline"
    );
});
