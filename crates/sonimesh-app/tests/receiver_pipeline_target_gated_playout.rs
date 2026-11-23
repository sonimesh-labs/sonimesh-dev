//! Target-gated playout driven by caller-owned arrival-pending signals.
//!
//! These tests feed explicit pending-arrival signals into
//! `decode_next_with_arrivals_pending` and assert the alternating
//! signal semantics, the streak reset on a drained-arrival play, the
//! enable/disable/re-enable counter lifecycle, and the fact that an
//! empty-buffer gated tick still advances the scheduler counters.

use core::convert::Infallible;
use std::net::UdpSocket;
use std::sync::Arc;

use sonimesh_app::{
    AdaptiveTargetConfig, ReceiverStreamPipeline, UdpPollOutcome, UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, JitterMode, JitterTargetConfig, RealtimePcmPopOutcome,
    TargetGatedPlayoutConfig, realtime_pcm_queue,
};
use sonimesh_diagnostics::{ExportSessionSnapshot, StreamCounters, build_export_bundle};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const JITTER_CAPACITY: usize = 4;
const MAX_ENCODED_BYTES: usize = 8;
const PCM_SAMPLES_PER_BLOCK: usize = 4;

type Pipeline =
    ReceiverStreamPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;
type UdpPipeline =
    UdpReceiverPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;

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

fn adaptive_pipeline(counters: Arc<StreamCounters>) -> (Pipeline, MediaSealer, SessionSecret) {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let pipeline = Pipeline::new_with_adaptive_target(
        &secret,
        context(1),
        ByteDecoder,
        producer,
        counters,
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source: Box::new(|| 0),
        },
    )
    .unwrap();
    let sealer = MediaSealer::new(&secret, context(1)).unwrap();
    (pipeline, sealer, secret)
}

/// Admits one sealed packet into the non-UDP pipeline without decoding it.
fn admit(pipeline: &mut Pipeline, sealer: &mut MediaSealer) {
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    let mut plaintext = [0_u8; MAX_ENCODED_BYTES];
    assert_eq!(
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
}

/// Polls until the loopback socket drains, returning whether any datagram arrived.
fn poll_until_empty(pipeline: &mut UdpPipeline) -> bool {
    let mut received = false;
    loop {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => break received,
            Ok(UdpPollOutcome::Processed { .. }) => received = true,
            other => panic!("unexpected poll outcome: {other:?}"),
        }
    }
}

#[test]
fn alternating_pending_signals_gate_and_drain_below_target() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut sealer, _secret) = adaptive_pipeline(Arc::clone(&counters));
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
        .unwrap();

    // One admitted frame sits below the balanced target (3), but the caller
    // reports no pending arrivals, so the tick must decode without forcing.
    admit(&mut pipeline, &mut sealer);
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(false).unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (0, 0),
        "a drained-arrival play below target is never forced"
    );

    // With one frame in flight the gate holds two ticks, then the expired
    // deadline forces the below-target play.
    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::Decoded { sequence: 1, .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (2, 1)
    );

    // Another drained-arrival play must neither force nor extend the gate
    // streak, so a fresh in-flight signal gets a full new deadline.
    admit(&mut pipeline, &mut sealer);
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(false).unwrap(),
        DecodeStep::Decoded { sequence: 2, .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (2, 1),
        "a drained-arrival play must not add gate telemetry"
    );
    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    let third = pipeline.decode_next_with_arrivals_pending(true).unwrap();
    assert!(
        matches!(third, DecodeStep::Decoded { sequence: 3, .. }),
        "the reset streak must permit the full deadline before forcing"
    );
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (4, 2)
    );
}

#[test]
fn enable_disable_reenable_resets_scheduler_counters_each_time() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut sealer, secret) = adaptive_pipeline(Arc::clone(&counters));

    // Enable, gate one below-target tick, and disable: both counters clear
    // and the next gated call decodes immediately.
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 4 })
        .unwrap();
    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(counters.snapshot().target_gated_ticks, 1);
    pipeline.disable_target_gated_playout();
    assert_eq!(
        counters.snapshot().target_gated_ticks,
        0,
        "disable must clear the scheduler counters"
    );
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));

    // Re-enable: the counters restart from zero and a fresh below-target
    // tick gates again with a full new deadline.
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 4 })
        .unwrap();
    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(counters.snapshot().target_gated_ticks, 1);

    // A stream reset retires the whole target epoch, including the enabled
    // scheduler's streak and counters, while gating stays active.
    pipeline.rotate(&secret, context(2), 0).unwrap();
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (0, 0),
        "rotation must retire the enabled scheduler telemetry"
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady,
        "the fresh epoch must keep gating below-target ticks"
    );
}

#[test]
fn empty_buffer_gated_ticks_still_advance_the_scheduler() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut sealer, _secret) = adaptive_pipeline(Arc::clone(&counters));
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
        .unwrap();

    // With no frames buffered at all, a pending-arrival tick still counts as
    // a gated tick because the empty buffer is below the target.
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(counters.snapshot().target_gated_ticks, 1);

    // Admit one frame and keep arrivals pending: the streak continues from
    // the empty-buffer tick and the deadline forces the decode.
    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (2, 1)
    );

    // A deadline-forced play on an empty buffer must conceal instead of
    // blocking the scheduling loop.
    let concealed = pipeline.decode_next_with_arrivals_pending(false).unwrap();
    assert!(
        matches!(concealed, DecodeStep::Concealed { sequence: 1, .. }),
        "the forced empty-buffer play must conceal, got {concealed:?}"
    );
}

#[test]
fn udp_wrapper_accepts_explicit_arrival_signals_after_socket_polls() {
    let counters = Arc::new(StreamCounters::default());
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, mut consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let mut pipeline = UdpPipeline::bind_with_adaptive_target(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context(1),
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source: Box::new(|| 0),
        },
    )
    .unwrap();
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
        .unwrap();

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&secret, context(1)).unwrap();
    let destination = pipeline.local_addr().unwrap();

    // Feed an explicit pending signal after the first delivery, then switch
    // to the drain signal and verify immediate, non-forced playout.
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    sender.send_to(&packet.datagram, destination).unwrap();
    assert!(
        poll_until_empty(&mut pipeline),
        "the datagram must be admitted"
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert!(
        !poll_until_empty(&mut pipeline),
        "the socket must be drained"
    );
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(false).unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let mut output = [f32::NAN; PCM_SAMPLES_PER_BLOCK];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { .. }
    ));

    // Deliver a second frame and supply a persistent pending signal: the gate
    // must hold until the deadline forces below-target playout.
    let packet = sealer.seal(0, 960, &[9]).unwrap();
    sender.send_to(&packet.datagram, destination).unwrap();
    assert!(
        poll_until_empty(&mut pipeline),
        "the second datagram must be admitted"
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert!(matches!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::Decoded { sequence: 1, .. }
    ));
    let snapshot = counters.snapshot();
    assert_eq!(
        (snapshot.target_gated_ticks, snapshot.target_forced_playouts),
        (3, 1)
    );
}

#[test]
fn exported_bundle_carries_the_scheduler_counters() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut sealer, _secret) = adaptive_pipeline(Arc::clone(&counters));
    pipeline
        .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 2 })
        .unwrap();

    admit(&mut pipeline, &mut sealer);
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );
    assert_eq!(
        pipeline.decode_next_with_arrivals_pending(true).unwrap(),
        DecodeStep::NotReady
    );

    let snapshot = counters.snapshot();
    let bundle = build_export_bundle(&ExportSessionSnapshot {
        software_version: "0.1.0",
        endpoint: None,
        platform_format: None,
        buffer_policy: None,
        session_phase: None,
        streams: std::slice::from_ref(&snapshot),
    })
    .unwrap();
    assert!(bundle.contains("\"target_gated_ticks\":\"2\""));
    assert!(bundle.contains("\"target_forced_playouts\":\"0\""));
}
