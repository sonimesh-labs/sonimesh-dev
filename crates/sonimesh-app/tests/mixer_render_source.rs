//! Phase 2 mixer output wiring tests: callback source plus control-domain
//! membership controller.
//!
//! ADR 0015 requires the app-side boundary to activate slots from decoded
//! streams and to publish gain/mute changes from the control domain. These
//! tests drive the real `MixerRenderSource` and `MixerMembershipController`
//! pair: streams are admitted through the controller, their producers feed the
//! slot queues, and renders must reflect membership, format, admission, and
//! limiter guarantees.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    GainControlHandle, MixerAdmissionError, MixerMembershipController, MixerRenderSource,
    ReceiverStreamPipeline, SourcePcmFormat,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, MAX_MIXER_COMMAND_QUEUE, RealtimePcmProducer, RealtimePcmPushOutcome,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, RenderSource, SampleFormat};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const SLOTS: usize = 4;
const SAMPLES_PER_BLOCK: usize = 8;
const TOLERANCE: f32 = 1e-6;

fn output_config() -> OutputConfig {
    OutputConfig {
        sample_rate_hz: 48_000,
        channels: 1,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: Some(480),
        maximum_callback_frames: 480,
    }
}

fn output_format() -> SourcePcmFormat {
    SourcePcmFormat::new(48_000, 1).unwrap()
}

/// Feeds `blocks` whole blocks of constant `value` through a slot producer.
fn push_constant(producer: &mut RealtimePcmProducer<SAMPLES_PER_BLOCK>, value: f32, blocks: usize) {
    for _ in 0..blocks {
        assert_eq!(
            producer.push(&[value; SAMPLES_PER_BLOCK]),
            RealtimePcmPushOutcome::Inserted
        );
    }
}

/// Renders `blocks` blocks through the mixer source and returns the samples.
fn render(source: &mut MixerRenderSource<SLOTS, SAMPLES_PER_BLOCK>, blocks: usize) -> Vec<f32> {
    let mut output = vec![0.0; blocks * SAMPLES_PER_BLOCK];
    source.render(&mut output);
    output
}

fn controller(
    source: &MixerRenderSource<SLOTS, SAMPLES_PER_BLOCK>,
) -> MixerMembershipController<SLOTS, SAMPLES_PER_BLOCK> {
    MixerMembershipController::new(source.membership(), 16, output_format()).unwrap()
}

fn admit(
    controller: &mut MixerMembershipController<SLOTS, SAMPLES_PER_BLOCK>,
    gain: GainControlHandle,
) -> (usize, RealtimePcmProducer<SAMPLES_PER_BLOCK>) {
    let stream = controller
        .admit_stream(output_format(), gain, None)
        .unwrap();
    (stream.slot, stream.producer)
}

#[test]
fn admitted_streams_mix_on_the_next_render() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let (slot_a, mut producer_a) = admit(&mut membership, GainControlHandle::new());
    assert_eq!(slot_a, 0);
    assert_eq!(
        membership.pending_commands(),
        1,
        "admission waits for a render"
    );

    push_constant(&mut producer_a, 0.25, 2);
    let (slot_b, mut producer_b) = admit(&mut membership, GainControlHandle::new());
    assert_eq!(slot_b, 1);
    push_constant(&mut producer_b, 0.5, 2);

    let output = render(&mut source, 2);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.75).abs() <= TOLERANCE),
        "first render after admission must mix both slots"
    );
    assert_eq!(membership.pending_commands(), 0);
    assert_eq!(membership.active(), 2);
    assert!(membership.is_active(0));
    assert!(membership.is_active(1));
}

#[test]
fn removed_stream_renders_silence() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let (slot_a, mut producer_a) = admit(&mut membership, GainControlHandle::new());
    let (_, mut producer_b) = admit(&mut membership, GainControlHandle::new());
    push_constant(&mut producer_a, 0.25, 2);
    push_constant(&mut producer_b, 0.5, 2);
    assert!(
        render(&mut source, 2)
            .iter()
            .all(|sample| (*sample - 0.75).abs() <= TOLERANCE)
    );

    membership.remove_stream(slot_a).unwrap();
    push_constant(&mut producer_b, 0.5, 2);
    let output = render(&mut source, 2);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.5).abs() <= TOLERANCE),
        "removed slot must render silence"
    );
    assert!(!membership.is_active(slot_a));
}

#[test]
fn admit_then_remove_before_render_reuses_the_freed_slot() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let (slot_a, _) = admit(&mut membership, GainControlHandle::new());
    assert_eq!(slot_a, 0);
    membership.remove_stream(slot_a).unwrap();

    // Both commands drain FIFO on one render, so the re-admission can reuse
    // the slot that the earlier activation vacated.
    let (slot_b, mut producer_b) = admit(&mut membership, GainControlHandle::new());
    assert_eq!(slot_b, 0);
    push_constant(&mut producer_b, 0.75, 2);
    let output = render(&mut source, 2);
    assert!(
        output
            .iter()
            .all(|sample| (*sample - 0.75).abs() <= TOLERANCE)
    );
    assert_eq!(membership.active(), 1);
    assert!(membership.is_active(0));
}

#[test]
fn format_mismatch_fails_closed() {
    let source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);

    assert!(matches!(
        membership.admit_stream(
            SourcePcmFormat::new(44_100, 1).unwrap(),
            GainControlHandle::new(),
            None,
        ),
        Err(MixerAdmissionError::FormatMismatch)
    ));
    assert!(matches!(
        membership.admit_stream(
            SourcePcmFormat::new(48_000, 2).unwrap(),
            GainControlHandle::new(),
            None,
        ),
        Err(MixerAdmissionError::FormatMismatch)
    ));
    assert_eq!(membership.active(), 0);
    assert_eq!(membership.pending_commands(), 0);
}

#[test]
fn slot_table_full_fails_closed_in_the_control_domain() {
    let source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    for _ in 0..SLOTS {
        admit(&mut membership, GainControlHandle::new());
    }
    assert!(matches!(
        membership.admit_stream(output_format(), GainControlHandle::new(), None),
        Err(MixerAdmissionError::SlotTableFull)
    ));
    assert_eq!(membership.active(), SLOTS);
}

#[test]
fn command_queue_full_is_retryable() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);

    // Fill the bounded command queue with stale removes without rendering.
    for _ in 0..MAX_MIXER_COMMAND_QUEUE / SLOTS {
        for slot in 0..SLOTS {
            membership.remove_stream(slot).unwrap();
        }
    }
    assert!(matches!(
        membership.remove_stream(0),
        Err(MixerAdmissionError::CommandQueueFull)
    ));
    assert_eq!(membership.pending_commands(), MAX_MIXER_COMMAND_QUEUE);

    // A render drains every queued stale remove and stays silent.
    let output = render(&mut source, 1);
    assert!(output.iter().all(|sample| sample.abs() <= TOLERANCE));
    assert_eq!(membership.pending_commands(), 0);
    assert_eq!(membership.active(), 0);
}

#[test]
fn output_stays_within_the_limiter_ceiling() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    let (_, mut producer_a) = admit(&mut membership, GainControlHandle::new());
    let (_, mut producer_b) = admit(&mut membership, GainControlHandle::new());
    push_constant(&mut producer_a, 0.8, 2);
    push_constant(&mut producer_b, 0.8, 2);

    let output = render(&mut source, 2);
    assert!(
        output
            .iter()
            .all(|sample| sample.is_finite() && *sample <= 1.0 + TOLERANCE),
        "mixed sum above full scale must be pulled back inside the ceiling"
    );
}

#[test]
fn control_domain_gain_and_mute_affect_only_the_target_stream() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership =
        MixerMembershipController::new(source.membership(), 128, output_format()).unwrap();
    let gain_a = GainControlHandle::new();
    let gain_b = GainControlHandle::new();
    let (_, mut producer_a) = admit(&mut membership, gain_a.clone());
    let (_, mut producer_b) = admit(&mut membership, gain_b.clone());

    // Feed and render twice so the per-slot gain ramps fully settle.
    for _ in 0..2 {
        push_constant(&mut producer_a, 0.25, 16);
        push_constant(&mut producer_b, 0.5, 16);
    }
    assert!(
        render(&mut source, 32)
            .iter()
            .all(|sample| (*sample - 0.75).abs() <= TOLERANCE)
    );

    gain_a.set_gain(0.5).unwrap();
    for _ in 0..2 {
        push_constant(&mut producer_a, 0.25, 16);
        push_constant(&mut producer_b, 0.5, 16);
    }
    let output = render(&mut source, 32);
    let last = output[output.len() - SAMPLES_PER_BLOCK..].to_vec();
    assert!(
        last.iter()
            .all(|sample| (*sample - 0.625).abs() <= TOLERANCE),
        "published gain must scale only stream A"
    );

    gain_a.set_muted(true);
    for _ in 0..2 {
        push_constant(&mut producer_a, 0.25, 16);
        push_constant(&mut producer_b, 0.5, 16);
    }
    let output = render(&mut source, 32);
    let last = output[output.len() - SAMPLES_PER_BLOCK..].to_vec();
    assert!(
        last.iter().all(|sample| (*sample - 0.5).abs() <= TOLERANCE),
        "published mute must silence only stream A"
    );
}

#[test]
fn out_of_range_remove_fails_fast() {
    let source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership = controller(&source);
    assert!(matches!(
        membership.remove_stream(SLOTS),
        Err(MixerAdmissionError::SlotOutOfRange)
    ));
}

#[test]
fn per_slot_counters_record_underflow_on_the_affected_slot() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership: MixerMembershipController<SLOTS, SAMPLES_PER_BLOCK> =
        MixerMembershipController::new(source.membership(), 16, output_format()).unwrap();
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let stream_a = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_a)),
        )
        .unwrap();
    let stream_b = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_b)),
        )
        .unwrap();
    let mut producer_a = stream_a.producer;
    let mut producer_b = stream_b.producer;

    // Feed each slot one block; the over-run render makes both underflow once.
    push_constant(&mut producer_a, 0.25, 1);
    push_constant(&mut producer_b, 0.5, 1);
    render(&mut source, 2);

    assert_eq!(counters_a.snapshot().pcm_underflows, 1);
    assert_eq!(counters_b.snapshot().pcm_underflows, 1);
    assert_eq!(counters_a.snapshot().pcm_refill_limit_hits, 0);
    assert_eq!(counters_b.snapshot().pcm_discontinuities, 0);
}

/// Decoder that fills each whole PCM block with constant 1.0 samples.
struct FullBlockDecoder;

impl FrameDecoder for FullBlockDecoder {
    type Error = Infallible;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(1.0);
        Ok(output.len())
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(output.len())
    }
}

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        1,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

fn full_block_pipeline(
    counters: Arc<StreamCounters>,
) -> ReceiverStreamPipeline<FullBlockDecoder, 4, 8, SAMPLES_PER_BLOCK> {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
    ReceiverStreamPipeline::new(&secret, context(), FullBlockDecoder, producer, counters, 0)
        .unwrap()
}

#[test]
fn one_stream_failure_leaves_other_slots_rendering() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership: MixerMembershipController<SLOTS, SAMPLES_PER_BLOCK> =
        MixerMembershipController::new(source.membership(), 64, output_format()).unwrap();
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut pipeline_a = full_block_pipeline(Arc::clone(&counters_a));
    let mut pipeline_b = full_block_pipeline(Arc::clone(&counters_b));

    let stream_a = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_a)),
        )
        .unwrap();
    let stream_b = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_b)),
        )
        .unwrap();
    pipeline_a.replace_pcm_producer(stream_a.producer);
    pipeline_b.replace_pcm_producer(stream_b.producer);

    // Stream A decodes one authenticated full block of 1.0 into its slot.
    let mut sealer_a = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer_a.seal(0, 0, &[1]).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline_a
        .receive(&packet.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline_a.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 0,
            samples: SAMPLES_PER_BLOCK,
            ..
        }
    ));

    // Stream B's network path fails: a packet sealed under a different secret
    // cannot authenticate, so nothing reaches its slot.
    let mut wrong_sealer = MediaSealer::new(&SessionSecret::new([0xB5; 32]), context()).unwrap();
    let packet = wrong_sealer.seal(0, 0, &[1]).unwrap();
    assert!(
        pipeline_b
            .receive(&packet.datagram, &mut plaintext)
            .is_err()
    );

    // The first render applies both admissions, then mixes: A contributes one
    // 1.0 block and underflows the tail chunk, while B underflows every chunk.
    // B's failure never blocks or resets A.
    let output = render(&mut source, 2);
    assert!(
        output[..SAMPLES_PER_BLOCK]
            .iter()
            .all(|sample| (*sample - 1.0).abs() <= TOLERANCE),
        "healthy stream must keep rendering after the other stream fails"
    );
    assert!(
        output[SAMPLES_PER_BLOCK..]
            .iter()
            .all(|sample| sample.abs() <= TOLERANCE)
    );

    assert_eq!(counters_a.snapshot().pcm_underflows, 1);
    assert_eq!(counters_b.snapshot().pcm_underflows, 2);
    assert!(counters_b.snapshot().rejected_packets >= 1);
    assert_eq!(counters_a.snapshot().pcm_discontinuities, 0);
    assert_eq!(counters_b.snapshot().pcm_discontinuities, 0);
}

/// Decoder whose authenticated payload always fails to decode.
struct FailingDecoder;

/// Injected decoder failure for cross-slot isolation tests.
#[derive(Debug, Clone, Copy)]
struct DecodeFailure;

impl std::fmt::Display for DecodeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("injected decode failure")
    }
}

impl std::error::Error for DecodeFailure {}

impl FrameDecoder for FailingDecoder {
    type Error = DecodeFailure;

    fn decode(&mut self, _encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Err(DecodeFailure)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(output.len())
    }
}

fn failing_pipeline(
    counters: Arc<StreamCounters>,
) -> ReceiverStreamPipeline<FailingDecoder, 4, 8, SAMPLES_PER_BLOCK> {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<SAMPLES_PER_BLOCK>(4).unwrap();
    ReceiverStreamPipeline::new(&secret, context(), FailingDecoder, producer, counters, 0).unwrap()
}

#[test]
fn one_stream_decode_failure_leaves_other_slots_rendering() {
    let mut source = MixerRenderSource::new(&output_config()).unwrap();
    let mut membership: MixerMembershipController<SLOTS, SAMPLES_PER_BLOCK> =
        MixerMembershipController::new(source.membership(), 64, output_format()).unwrap();
    let counters_a = Arc::new(StreamCounters::default());
    let counters_b = Arc::new(StreamCounters::default());
    let mut pipeline_a = full_block_pipeline(Arc::clone(&counters_a));
    let mut pipeline_b = failing_pipeline(Arc::clone(&counters_b));

    let stream_a = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_a)),
        )
        .unwrap();
    let stream_b = membership
        .admit_stream(
            output_format(),
            GainControlHandle::new(),
            Some(Arc::clone(&counters_b)),
        )
        .unwrap();
    pipeline_a.replace_pcm_producer(stream_a.producer);
    pipeline_b.replace_pcm_producer(stream_b.producer);

    // Stream A decodes one authenticated full block of 1.0 into its slot.
    let mut sealer_a = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer_a.seal(0, 0, &[1]).unwrap();
    let mut plaintext = [0_u8; 8];
    pipeline_a
        .receive(&packet.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline_a.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 0,
            samples: SAMPLES_PER_BLOCK,
            ..
        }
    ));

    // Stream B authenticates a packet but its decoder rejects the payload, so
    // only B's decode path resets; nothing reaches B's mixer slot.
    let mut sealer_b = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer_b.seal(0, 0, &[1]).unwrap();
    pipeline_b
        .receive(&packet.datagram, &mut plaintext)
        .unwrap();
    assert!(pipeline_b.decode_next().is_err());

    let output = render(&mut source, 2);
    assert!(
        output[..SAMPLES_PER_BLOCK]
            .iter()
            .all(|sample| (*sample - 1.0).abs() <= TOLERANCE),
        "healthy stream must keep rendering after the other stream's decoder fails"
    );
    assert!(
        output[SAMPLES_PER_BLOCK..]
            .iter()
            .all(|sample| sample.abs() <= TOLERANCE)
    );

    assert_eq!(counters_a.snapshot().pcm_underflows, 1);
    assert_eq!(counters_b.snapshot().pcm_underflows, 2);
    assert!(counters_b.snapshot().decode_errors >= 1);
    assert!(counters_b.snapshot().stream_resets >= 1);
    assert_eq!(counters_a.snapshot().pcm_discontinuities, 0);
}
