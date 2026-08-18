//! Adaptive-target controller wiring through a real protected packet stream.
//!
//! These tests feed authenticated media packets through the receiver pipeline
//! with a scripted monotonic arrival clock, proving that the arrival-jitter
//! estimator and the adaptive target controller run on the live packet stream
//! and that their state flows into the shared diagnostics counters.

use core::convert::Infallible;
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

use sonimesh_app::{
    AdaptiveTargetConfig, FreshSessionInstallError, ReceiverStreamPipeline, UdpPollOutcome,
    UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, JitterMode, JitterTargetConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::{ExportSessionSnapshot, StreamCounters, build_export_bundle};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const JITTER_CAPACITY: usize = 4;
const MAX_ENCODED_BYTES: usize = 8;
const PCM_SAMPLES_PER_BLOCK: usize = 4;

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

type Pipeline =
    ReceiverStreamPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;
type UdpPipeline =
    UdpReceiverPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;

fn context() -> MediaKeyContext {
    context_for(9)
}

fn context_for(stream_id: u32) -> MediaKeyContext {
    context_for_session(0x0102_0304_0506_0708, stream_id)
}

fn context_for_session(session_id: u64, stream_id: u32) -> MediaKeyContext {
    MediaKeyContext::new(session_id, stream_id, 1, MediaDirection::AndroidToDesktop).unwrap()
}

type TickSource = Box<dyn FnMut() -> u64>;

fn tick_source(ticks: &[u64]) -> (TickSource, Arc<Mutex<VecDeque<u64>>>) {
    let queue = Arc::new(Mutex::new(ticks.iter().copied().collect::<VecDeque<_>>()));
    let source = {
        let queue = Arc::clone(&queue);
        Box::new(move || {
            queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("arrival tick sequence exhausted")
        })
    };
    (source, queue)
}

fn adaptive_pipeline(
    ticks: &[u64],
    counters: Arc<StreamCounters>,
) -> (Pipeline, MediaSealer, Arc<Mutex<VecDeque<u64>>>) {
    adaptive_pipeline_for_context(context(), ticks, counters)
}

fn adaptive_pipeline_for_context(
    context: MediaKeyContext,
    ticks: &[u64],
    counters: Arc<StreamCounters>,
) -> (Pipeline, MediaSealer, Arc<Mutex<VecDeque<u64>>>) {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, _consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let (tick_source, queue) = tick_source(ticks);
    let pipeline = Pipeline::new_with_adaptive_target(
        &secret,
        context,
        ByteDecoder,
        producer,
        counters,
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source,
        },
    )
    .unwrap();
    let sealer = MediaSealer::new(&secret, context).unwrap();
    (pipeline, sealer, queue)
}

fn send(pipeline: &mut Pipeline, sealer: &mut MediaSealer, sample_timestamp: u64) {
    let packet = sealer.seal(0, sample_timestamp, &[7]).unwrap();
    let mut plaintext = [0_u8; MAX_ENCODED_BYTES];
    assert_eq!(
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
    // Drain the admitted frame so the fixed-capacity buffer never fills and
    // forces a resynchronization that would drop the playout state.
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { .. }
    ));
}

fn send_udp_and_decode(
    pipeline: &mut UdpPipeline,
    sender: &UdpSocket,
    destination: SocketAddr,
    sealer: &mut MediaSealer,
    sample_timestamp: u64,
    consumer: &mut RealtimePcmConsumer<PCM_SAMPLES_PER_BLOCK>,
) {
    let packet = sealer.seal(0, sample_timestamp, &[7]).unwrap();
    sender.send_to(&packet.datagram, destination).unwrap();
    let outcome = loop {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => thread::yield_now(),
            other => break other,
        }
    };
    assert!(matches!(outcome, Ok(UdpPollOutcome::Processed { .. })));
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { .. }
    ));
    let mut output = [f32::NAN; PCM_SAMPLES_PER_BLOCK];
    let _ = consumer.pop_into(&mut output);
}

fn target_state(counters: &StreamCounters) -> (u16, u16, u16, u32, u32, u32) {
    let snapshot = counters.snapshot();
    (
        snapshot.target_min_frames,
        snapshot.target_max_frames,
        snapshot.target_frames,
        snapshot.target_smoothed_jitter_ms,
        snapshot.target_raised_steps,
        snapshot.target_lowered_steps,
    )
}

#[test]
fn pipeline_feeds_the_controller_and_publishes_the_target_state() {
    let counters = Arc::new(StreamCounters::default());
    // Four 20 ms frames at perfect cadence, then arrival slips of 100 ms per
    // frame so the controller sees 80 ms of sustained jitter.
    let mut ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = ticks[3];
    for _ in 0..32 {
        next += 100;
        ticks.push(next);
    }
    let (mut pipeline, mut sealer, _queue) = adaptive_pipeline(&ticks, Arc::clone(&counters));

    for frame in 0_u64..4 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    // Balanced 40-70 ms at 20 ms frames: min 2, base 3, max 4 frames. Perfect
    // cadence contributes zero jitter, so the target stays at base.
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "perfect cadence must keep the target at base with no steps"
    );

    for frame in 4_u64..36 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    let (min, max, target, smoothed, raised, _lowered) = target_state(&counters);
    assert_eq!((min, max), (2, 4));
    assert!(
        smoothed > 0,
        "sustained arrival slip must move the smoothed jitter estimate"
    );
    assert!(
        raised >= 1,
        "sustained high jitter must raise the target at least once"
    );
    assert!(target > 3, "the target must leave the base under pressure");
}

#[test]
fn hard_resynchronize_retires_and_republishes_the_target_epoch() {
    let counters = Arc::new(StreamCounters::default());
    let mut ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = ticks[3];
    for _ in 0..12 {
        next += 100;
        ticks.push(next);
    }
    // A fresh 20 ms cadence after the reset.
    for _ in 0..3 {
        next += 20;
        ticks.push(next);
    }
    let (mut pipeline, mut sealer, _queue) = adaptive_pipeline(&ticks, Arc::clone(&counters));

    for frame in 0_u64..16 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    assert!(target_state(&counters).3 > 0, "jitter must be visible");

    pipeline.hard_resynchronize();
    assert_eq!(
        target_state(&counters),
        (0, 0, 0, 0, 0, 0),
        "a stream-local retirement must clear the published target epoch"
    );

    // The controller restarts from a fresh baseline: the next packet only
    // establishes it, and following perfect-cadence packets keep the target at
    // base with no steps.
    for frame in 16_u64..19 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "after a reset a fresh epoch republishes the configured water levels"
    );
}

#[test]
fn jitter_resynchronization_starts_a_new_arrival_baseline_at_the_resync_packet() {
    let counters = Arc::new(StreamCounters::default());
    // Leave the first four frames buffered so the fifth frame forces the
    // bounded jitter buffer to resynchronize. The resynchronizing packet must
    // become the first arrival baseline of the new adaptive epoch.
    let (mut pipeline, mut sealer, _queue) =
        adaptive_pipeline(&[10, 30, 50, 70, 170, 270], Arc::clone(&counters));
    let mut admit = |sequence: u8, sample_timestamp: u64| {
        let packet = sealer
            .seal(sequence, sample_timestamp, &[7])
            .expect("test packet seals");
        let mut plaintext = [0_u8; MAX_ENCODED_BYTES];
        pipeline
            .receive(&packet.datagram, &mut plaintext)
            .expect("authenticated packet admits")
    };

    for sequence in 0..4 {
        assert_eq!(
            admit(sequence, u64::from(sequence) * 960),
            InsertOutcome::Inserted
        );
    }
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "the populated epoch must still report the balanced base"
    );

    assert_eq!(
        admit(4, 4 * 960),
        InsertOutcome::Resynchronized { dropped_frames: 4 }
    );
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "the resynchronizing packet must publish a clean new target epoch"
    );
    assert_eq!(
        counters.snapshot().jitter_resync_dropped_frames,
        4,
        "the resynchronization must publish its dropped-frame count"
    );

    // The next packet arrives 100 ms after the resync packet while advancing
    // only one 20 ms media frame. Observing it against the resync packet proves
    // that the new baseline was established immediately rather than one packet
    // late.
    assert_eq!(admit(5, 5 * 960), InsertOutcome::Inserted);
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 20, 0, 0),
        "the first post-resync interval must feed the new jitter estimate"
    );
}

#[test]
fn fresh_session_retires_target_only_after_a_valid_replacement() {
    let counters = Arc::new(StreamCounters::default());
    let mut ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = *ticks.last().unwrap();
    for _ in 0..12 {
        next += 100;
        ticks.push(next);
    }
    for _ in 0..3 {
        next += 20;
        ticks.push(next);
    }
    let (mut pipeline, mut sealer, _queue) = adaptive_pipeline(&ticks, Arc::clone(&counters));

    for frame in 0_u64..16 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    let populated = target_state(&counters);
    assert!(
        populated.3 > 0,
        "the original session must accumulate visible target jitter"
    );

    let secret = SessionSecret::new([0xA5; 32]);
    assert_eq!(
        pipeline.install_fresh_session(&secret, context(), 0),
        Err(FreshSessionInstallError::ReusedSessionId),
        "a replacement must not reuse the active session ID"
    );
    assert_eq!(
        target_state(&counters),
        populated,
        "a rejected replacement must preserve the active target epoch"
    );

    let fresh_context = context_for_session(0x0A0B_0C0D_0E0F_0102, 9);
    pipeline
        .install_fresh_session(&secret, fresh_context, 0)
        .unwrap();
    assert_eq!(
        target_state(&counters),
        (0, 0, 0, 0, 0, 0),
        "a valid fresh session must retire the replaced target epoch"
    );

    let mut fresh_sealer = MediaSealer::new(&secret, fresh_context).unwrap();
    for frame in 0_u64..3 {
        send(&mut pipeline, &mut fresh_sealer, frame * 960);
    }
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "the fresh session must rebuild its target from a new arrival baseline"
    );
}

#[test]
fn independent_stream_targets_and_resets_do_not_cross_contaminate() {
    let calm_counters = Arc::new(StreamCounters::default());
    let jitter_counters = Arc::new(StreamCounters::default());

    let calm_ticks = (1_u64..=36).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut jitter_ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = *jitter_ticks.last().unwrap();
    for _ in 0..32 {
        next += 100;
        jitter_ticks.push(next);
    }

    let (mut calm_pipeline, mut calm_sealer, _calm_queue) =
        adaptive_pipeline_for_context(context_for(9), &calm_ticks, Arc::clone(&calm_counters));
    let (mut jitter_pipeline, mut jitter_sealer, _jitter_queue) =
        adaptive_pipeline_for_context(context_for(10), &jitter_ticks, Arc::clone(&jitter_counters));

    for frame in 0_u64..36 {
        send(&mut calm_pipeline, &mut calm_sealer, frame * 960);
        send(&mut jitter_pipeline, &mut jitter_sealer, frame * 960);
    }

    let calm_before_reset = target_state(&calm_counters);
    let jitter_before_reset = target_state(&jitter_counters);
    assert_eq!(
        calm_before_reset,
        (2, 4, 3, 0, 0, 0),
        "a calm stream must retain its independent base target"
    );
    assert!(
        jitter_before_reset.3 > 0 && jitter_before_reset.4 >= 1,
        "the impaired stream must accumulate its own jitter response"
    );

    calm_pipeline.hard_resynchronize();
    assert_eq!(
        target_state(&calm_counters),
        (0, 0, 0, 0, 0, 0),
        "resetting one stream must retire only its target epoch"
    );
    assert_eq!(
        target_state(&jitter_counters),
        jitter_before_reset,
        "resetting one stream must not clear another stream's target history"
    );

    jitter_pipeline.hard_resynchronize();
    assert_eq!(
        target_state(&jitter_counters),
        (0, 0, 0, 0, 0, 0),
        "the second stream can retire its own target epoch independently"
    );
    assert_eq!(
        target_state(&calm_counters),
        (0, 0, 0, 0, 0, 0),
        "the first stream remains retired after the second reset"
    );
}

#[test]
fn backward_arrival_ticks_are_skipped_without_corrupting_the_baseline() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut sealer, queue) = adaptive_pipeline(&[10], Arc::clone(&counters));

    // First packet establishes the baseline only.
    send(&mut pipeline, &mut sealer, 0);
    assert_eq!(target_state(&counters), (2, 4, 3, 0, 0, 0));

    // A backward tick is skipped without advancing the estimator or controller.
    queue.lock().unwrap().push_back(5);
    send(&mut pipeline, &mut sealer, 960);
    assert_eq!(target_state(&counters), (2, 4, 3, 0, 0, 0));

    // The next forward tick measures from the preserved baseline pair
    // (10 ticks, timestamp 0): 20 ticks arrive against a 40 ms sample delta,
    // so the deviation is 20 ms and the smoothed estimate moves to 5 ms. Had
    // the backward tick replaced the baseline, the deviation would instead be
    // 5 ms, so this value proves the original baseline survived the skip.
    queue.lock().unwrap().push_back(30);
    send(&mut pipeline, &mut sealer, 1920);
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 5, 0, 0),
        "a skipped backward tick must not corrupt later measurements"
    );
}

#[test]
fn receives_authenticated_udp_media_into_the_adaptive_controller() {
    use std::net::UdpSocket;
    use std::thread;

    use sonimesh_app::{UdpPollOutcome, UdpReceiverPipeline};

    type UdpPipeline =
        UdpReceiverPipeline<ByteDecoder, JITTER_CAPACITY, MAX_ENCODED_BYTES, PCM_SAMPLES_PER_BLOCK>;

    let counters = Arc::new(StreamCounters::default());
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let (producer, _consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let (tick_source, _queue) = tick_source(&[10, 30]);
    let mut pipeline = UdpPipeline::bind_with_adaptive_target(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source,
        },
    )
    .unwrap();

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    sender
        .send_to(&packet.datagram, pipeline.local_addr().unwrap())
        .unwrap();

    let mut outcome = None;
    for _ in 0..1_000 {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => thread::yield_now(),
            other => {
                outcome = Some(other);
                break;
            }
        }
    }
    let outcome = outcome.expect("loopback datagram was not delivered");
    assert!(matches!(outcome, Ok(UdpPollOutcome::Processed { .. })));

    // The first packet only establishes the arrival baseline, but the target
    // configuration is already published for the active epoch.
    assert_eq!(target_state(&counters), (2, 4, 3, 0, 0, 0));
}

#[test]
fn udp_jitter_resynchronization_starts_a_new_arrival_baseline() {
    let counters = Arc::new(StreamCounters::default());
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let (producer, mut consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let (tick_source, _queue) = tick_source(&[10, 30, 50, 70, 170, 270]);
    let mut pipeline = UdpPipeline::bind_with_adaptive_target(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source,
        },
    )
    .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let source = sender.local_addr().unwrap();
    let destination = pipeline.local_addr().unwrap();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();

    for sequence in 0_u64..6 {
        let packet = sealer
            .seal(0, sequence * 960, &[7])
            .expect("test packet seals");
        sender.send_to(&packet.datagram, destination).unwrap();
        let outcome = loop {
            match pipeline.poll() {
                Ok(UdpPollOutcome::Empty) => thread::yield_now(),
                other => break other,
            }
        };
        let expected = if sequence == 4 {
            InsertOutcome::Resynchronized { dropped_frames: 4 }
        } else {
            InsertOutcome::Inserted
        };
        assert!(matches!(
            outcome,
            Ok(UdpPollOutcome::Processed {
                source: observed,
                admission,
            }) if observed == source && admission == expected
        ));
    }

    assert_eq!(pipeline.pinned_source(), Some(source));
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 20, 0, 0),
        "the UDP resync packet must be the immediate arrival baseline"
    );
    assert_eq!(
        counters.snapshot().jitter_resync_dropped_frames,
        4,
        "the UDP resynchronization must publish its dropped-frame count"
    );
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 4, .. }
    ));
    let mut output = [f32::NAN; PCM_SAMPLES_PER_BLOCK];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { .. }
    ));
}

#[test]
fn udp_fresh_session_resets_target_and_source_pin_together() {
    let counters = Arc::new(StreamCounters::default());
    let secret = SessionSecret::new([0xA5; 32]);
    let active_context = context();
    let mut ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = *ticks.last().unwrap();
    for _ in 0..12 {
        next += 100;
        ticks.push(next);
    }
    for _ in 0..3 {
        next += 20;
        ticks.push(next);
    }
    let (producer, mut consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(4).unwrap();
    let (tick_source, _queue) = tick_source(&ticks);
    let mut pipeline = UdpPipeline::bind_with_adaptive_target(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        active_context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
        AdaptiveTargetConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_CAPACITY),
            arrival_ticks_per_second: 1_000,
            tick_source,
        },
    )
    .unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&secret, active_context).unwrap();
    let destination = pipeline.local_addr().unwrap();
    for frame in 0_u64..16 {
        send_udp_and_decode(
            &mut pipeline,
            &sender,
            destination,
            &mut sealer,
            frame * 960,
            &mut consumer,
        );
    }
    let populated = target_state(&counters);
    assert!(
        populated.3 > 0,
        "the original UDP session must accumulate target jitter"
    );
    assert_eq!(
        pipeline.pinned_source(),
        Some(sender.local_addr().unwrap()),
        "the original session must bind the source endpoint"
    );

    assert_eq!(
        pipeline.install_fresh_session(&secret, context(), 0),
        Err(FreshSessionInstallError::ReusedSessionId),
        "a reused UDP session must be rejected"
    );
    assert_eq!(target_state(&counters), populated);
    assert_eq!(
        pipeline.pinned_source(),
        Some(sender.local_addr().unwrap()),
        "a rejected replacement must preserve the source pin"
    );

    let fresh_context = context_for_session(0x0A0B_0C0D_0E0F_0102, 9);
    pipeline
        .install_fresh_session(&secret, fresh_context, 0)
        .unwrap();
    assert_eq!(
        target_state(&counters),
        (0, 0, 0, 0, 0, 0),
        "a valid UDP fresh session must retire the target epoch"
    );
    assert_eq!(
        pipeline.pinned_source(),
        None,
        "a valid UDP fresh session must clear the source pin"
    );

    let mut fresh_sealer = MediaSealer::new(&secret, fresh_context).unwrap();
    for frame in 0_u64..3 {
        send_udp_and_decode(
            &mut pipeline,
            &sender,
            destination,
            &mut fresh_sealer,
            frame * 960,
            &mut consumer,
        );
    }
    assert_eq!(
        target_state(&counters),
        (2, 4, 3, 0, 0, 0),
        "the fresh UDP session must rebuild target state from a new baseline"
    );
    assert_eq!(
        pipeline.pinned_source(),
        Some(sender.local_addr().unwrap()),
        "the fresh session must rebind the authenticated source"
    );
}

#[test]
fn published_target_state_reaches_the_export_bundle() {
    let counters = Arc::new(StreamCounters::default());
    let mut ticks = (1_u64..=4).map(|frame| frame * 20).collect::<Vec<_>>();
    let mut next = ticks[3];
    for _ in 0..32 {
        next += 100;
        ticks.push(next);
    }
    let (mut pipeline, mut sealer, _queue) = adaptive_pipeline(&ticks, Arc::clone(&counters));

    for frame in 0_u64..36 {
        send(&mut pipeline, &mut sealer, frame * 960);
    }
    let (min, max, target, smoothed, raised, lowered) = target_state(&counters);
    assert!(smoothed > 0 && raised >= 1, "the controller must respond");

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

    // The full chain is proven: live packets drive the controller, the
    // controller publishes through the stream counters, and the export
    // bundle carries the same values verbatim.
    assert!(bundle.contains(&format!("\"target_min_frames\":{min}")));
    assert!(bundle.contains(&format!("\"target_max_frames\":{max}")));
    assert!(bundle.contains(&format!("\"target_frames\":{target}")));
    assert!(bundle.contains(&format!("\"target_raised_steps\":{raised}")));
    assert!(bundle.contains(&format!("\"target_lowered_steps\":{lowered}")));

    let marker = "\"target_smoothed_jitter_ms\":";
    let start = bundle
        .find(marker)
        .expect("smoothed jitter must be exported");
    let tail = &bundle[start + marker.len()..];
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let exported_smoothed: u32 = digits.parse().expect("smoothed jitter must be numeric");
    assert_eq!(exported_smoothed, smoothed);
}
