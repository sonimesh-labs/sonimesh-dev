//! Cross-domain receiver pipeline tests with real media AEAD packets.

use core::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_app::{
    ControlStreamAction, ControlStreamCoordinator, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingSource, ReceiverStreamPipeline, SessionState,
};
use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, RealtimePcmPopOutcome,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::IngressError;
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaKeyContext,
    MediaSealer, SessionSecret, StartStream, envelope,
};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FailingDecoderError;

impl std::fmt::Display for FailingDecoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("test decoder failure")
    }
}

impl std::error::Error for FailingDecoderError {}

struct FailingDecoder {
    fail_next_decode: bool,
    reset_count: Arc<AtomicUsize>,
}

impl FrameDecoder for FailingDecoder {
    type Error = FailingDecoderError;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        if core::mem::replace(&mut self.fail_next_decode, false) {
            return Err(FailingDecoderError);
        }
        output[0] = f32::from(encoded[0]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }

    fn reset(&mut self) {
        self.reset_count.fetch_add(1, Ordering::Relaxed);
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

fn start_command(epoch: u16) -> Envelope {
    let context = context(epoch);
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: u64::from(epoch),
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: context.session_id,
            stream_id: context.stream_id,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: u32::from(epoch),
        })),
    }
}

fn trusted_session() -> SessionState {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    pairing
        .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.record_proof(&mut session, 101, PairingSource::new([2; 16]), true),
        Ok(PairingAttemptOutcome::Paired)
    );
    session
}

fn negotiate(coordinator: &mut ControlStreamCoordinator) {
    let hello = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "Lab sender".to_owned(),
        })),
    };
    assert!(matches!(
        coordinator.accept(&hello),
        Ok(ControlStreamAction::Negotiated(_))
    ));
}

#[test]
fn receives_authenticated_media_then_decodes_into_realtime_pcm() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::new(StreamCounters::default()),
        0,
    )
    .unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    let mut plaintext = [0_u8; 8];

    assert_eq!(
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
    assert!(plaintext.iter().all(|byte| *byte == 0));
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 0,
            discontinuity: false,
            discarded_stale_blocks: 0,
        }
    );
    assert!((output[0] - 7.0).abs() < f32::EPSILON);
}

#[test]
fn rotation_rejects_old_packets_and_retires_old_pcm() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let mut old_sealer = MediaSealer::new(&secret, initial).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let old_packet = old_sealer.seal(0, 0, &[1]).unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline
        .receive(&old_packet.datagram, &mut plaintext)
        .unwrap();
    pipeline.decode_next().unwrap();
    let next = context(3);
    pipeline.rotate(&secret, next, 0).unwrap();
    assert_eq!(counters.snapshot().queued_pcm_high_water_blocks, 0);

    plaintext.fill(0xFF);
    assert!(matches!(
        pipeline.receive(&old_packet.datagram, &mut plaintext),
        Err(sonimesh_app::ReceiverPipelineError::Ingress(
            IngressError::UnknownStream
        ))
    ));
    assert!(plaintext.iter().all(|byte| *byte == 0));

    let mut new_sealer = MediaSealer::new(&secret, next).unwrap();
    let new_packet = new_sealer.seal(0, 0, &[2]).unwrap();
    pipeline
        .receive(&new_packet.datagram, &mut plaintext)
        .unwrap();
    pipeline.decode_next().unwrap();

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            sequence: 0,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);
}

#[test]
fn control_epoch_rotation_commits_only_after_pipeline_context_rotates() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::new(StreamCounters::default()),
        0,
    )
    .unwrap();
    let mut coordinator =
        ControlStreamCoordinator::new(trusted_session(), &[Capability::FrameDuration20Ms]).unwrap();
    negotiate(&mut coordinator);
    let ControlStreamAction::Start {
        token: initial_token,
        request: initial_request,
    } = coordinator.accept(&start_command(2)).unwrap()
    else {
        panic!("expected initial start action");
    };
    coordinator
        .complete_start(initial_token, initial_request)
        .unwrap();

    let ControlStreamAction::Rotate {
        token,
        previous,
        next,
    } = coordinator.accept(&start_command(3)).unwrap()
    else {
        panic!("expected rotate action");
    };
    let next_context = next
        .identity
        .media_key_context(MediaDirection::AndroidToDesktop)
        .unwrap();
    pipeline.rotate(&secret, next_context, 0).unwrap();
    coordinator
        .complete_rotation(token, previous, next)
        .unwrap();

    assert_eq!(pipeline.context(), next_context);
    assert_eq!(coordinator.active(), Some(next.identity));
}

#[test]
fn pipeline_publishes_bounded_receive_decode_and_epoch_metrics() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let mut sealer = MediaSealer::new(&secret, initial).unwrap();
    let (producer, _consumer) = realtime_pcm_queue::<4>(1).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let _first = sealer.seal(0, 0, &[6]).unwrap();
    let packet = sealer.seal(0, 960, &[7]).unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Concealed { .. }
    ));
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { .. }
    ));
    assert!(matches!(
        pipeline.receive(&packet.datagram, &mut plaintext),
        Err(sonimesh_app::ReceiverPipelineError::Ingress(
            IngressError::Replay(_)
        ))
    ));
    assert!(matches!(
        pipeline.receive(&[0_u8], &mut plaintext),
        Err(sonimesh_app::ReceiverPipelineError::Ingress(
            IngressError::Parse(_)
        ))
    ));

    pipeline.rotate(&secret, context(3), 0).unwrap();
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.authenticated_packets, 2);
    assert_eq!(snapshot.duplicate_packets, 1);
    assert_eq!(snapshot.rejected_packets, 2);
    assert_eq!(snapshot.plc_frames, 1);
    assert_eq!(snapshot.pcm_overflows, 1);
    assert_eq!(snapshot.stream_resets, 1);
}

#[test]
fn pipeline_publishes_late_and_controlled_resynchronization_metrics() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let mut sealer = MediaSealer::new(&secret, initial).unwrap();
    let packet_zero = sealer.seal(0, 0, &[0]).unwrap();
    let packet_one = sealer.seal(0, 960, &[1]).unwrap();
    let packet_two = sealer.seal(0, 1_920, &[2]).unwrap();
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let late_counters = Arc::new(StreamCounters::default());
    let mut late_pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::clone(&late_counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    late_pipeline
        .receive(&packet_zero.datagram, &mut plaintext)
        .unwrap();
    late_pipeline.decode_next().unwrap();
    late_pipeline
        .receive(&packet_two.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        late_pipeline.decode_next().unwrap(),
        DecodeStep::Concealed { .. }
    ));
    assert_eq!(
        late_pipeline
            .receive(&packet_one.datagram, &mut plaintext)
            .unwrap(),
        InsertOutcome::Late
    );
    assert_eq!(late_counters.snapshot().late_packets, 1);

    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let reset_counters = Arc::new(StreamCounters::default());
    let mut reset_pipeline = ReceiverStreamPipeline::<ByteDecoder, 2, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        Arc::clone(&reset_counters),
        0,
    )
    .unwrap();
    reset_pipeline
        .receive(&packet_zero.datagram, &mut plaintext)
        .unwrap();
    assert_eq!(
        reset_pipeline
            .receive(&packet_two.datagram, &mut plaintext)
            .unwrap(),
        InsertOutcome::Resynchronized { dropped_frames: 1 }
    );
    assert_eq!(reset_counters.snapshot().stream_resets, 1);
}

#[test]
fn jitter_resynchronization_retires_old_pcm_but_keeps_the_new_packet() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = sealer.seal(0, 0, &[1]).unwrap();
    let buffered = sealer.seal(0, 960, &[2]).unwrap();
    let _skipped = sealer.seal(0, 1_920, &[0]).unwrap();
    let resync_point = sealer.seal(0, 2_880, &[3]).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 2, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&first.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    pipeline
        .receive(&buffered.datagram, &mut plaintext)
        .unwrap();
    assert_eq!(
        pipeline
            .receive(&resync_point.datagram, &mut plaintext)
            .unwrap(),
        InsertOutcome::Resynchronized { dropped_frames: 1 }
    );
    assert_eq!(counters.snapshot().queued_pcm_blocks, 0);
    assert_eq!(counters.snapshot().queued_pcm_high_water_blocks, 0);
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 3, .. }
    ));

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert!((output[0] - 3.0).abs() < f32::EPSILON);
    assert_eq!(counters.snapshot().jitter_resync_dropped_frames, 1);
    assert_eq!(counters.snapshot().stream_resets, 1);
}

#[test]
fn hard_resync_retires_only_the_current_stream_playout_state() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = sealer.seal(0, 0, &[1]).unwrap();
    let pending = sealer.seal(0, 960, &[2]).unwrap();
    let after_reset = sealer.seal(0, 1_920, &[3]).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&first.datagram, &mut plaintext).unwrap();
    pipeline.receive(&pending.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));

    pipeline.hard_resynchronize();

    assert_eq!(pipeline.decode_next().unwrap(), DecodeStep::NotReady);
    assert_eq!(counters.snapshot().queued_pcm_high_water_blocks, 0);
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 1,
        }
    );
    pipeline
        .receive(&after_reset.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 2, .. }
    ));
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            ..
        }
    ));
    assert!((output[0] - 3.0).abs() < f32::EPSILON);
    assert_eq!(counters.snapshot().stream_resets, 1);
}

#[test]
fn fresh_session_replaces_replay_and_playout_state_without_recreating_output() {
    let old_secret = SessionSecret::new([0xA5; 32]);
    let old_context = context(2);
    let fresh_secret = SessionSecret::new([0x5A; 32]);
    let fresh_context = MediaKeyContext::new(
        old_context.session_id + 1,
        old_context.stream_id,
        0,
        old_context.direction,
    )
    .unwrap();
    let mut old_sealer = MediaSealer::new(&old_secret, old_context).unwrap();
    let old_packet = old_sealer.seal(0, 0, &[1]).unwrap();
    let mut fresh_sealer = MediaSealer::new(&fresh_secret, fresh_context).unwrap();
    let fresh_packet = fresh_sealer.seal(0, 0, &[2]).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &old_secret,
        old_context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline
        .receive(&old_packet.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { .. }
    ));
    pipeline
        .install_fresh_session(&fresh_secret, fresh_context, 0)
        .unwrap();
    assert_eq!(pipeline.context(), fresh_context);
    assert!(
        pipeline
            .receive(&old_packet.datagram, &mut plaintext)
            .is_err()
    );
    pipeline
        .receive(&fresh_packet.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { .. }
    ));

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);
    assert_eq!(counters.snapshot().stream_resets, 1);
}

#[test]
fn fresh_session_rejects_reusing_the_active_session_id() {
    let secret = SessionSecret::new([0xA5; 32]);
    let active = context(2);
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        active,
        ByteDecoder,
        producer,
        Arc::new(StreamCounters::default()),
        0,
    )
    .unwrap();

    assert_eq!(
        pipeline.install_fresh_session(&secret, active, 0),
        Err(sonimesh_app::FreshSessionInstallError::ReusedSessionId)
    );
    assert_eq!(pipeline.context(), active);
}

#[test]
fn replacing_pcm_producer_restarts_only_the_output_side_of_one_stream() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = sealer.seal(0, 0, &[1]).unwrap();
    let after_replacement = sealer.seal(0, 960, &[2]).unwrap();
    let (producer, _old_consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&first.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let (replacement, mut replacement_consumer) = realtime_pcm_queue::<4>(4).unwrap();

    pipeline.replace_pcm_producer(replacement);

    assert_eq!(pipeline.decode_next().unwrap(), DecodeStep::NotReady);
    assert_eq!(counters.snapshot().queued_pcm_high_water_blocks, 0);
    pipeline
        .receive(&after_replacement.datagram, &mut plaintext)
        .unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 1, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        replacement_consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            sequence: 0,
            discontinuity: false,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);
    assert_eq!(counters.snapshot().stream_resets, 1);
}

#[test]
fn decoder_failure_retires_only_its_stream_and_accepts_the_next_packet() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = sealer.seal(0, 0, &[1]).unwrap();
    let second = sealer.seal(0, 960, &[2]).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let reset_count = Arc::new(AtomicUsize::new(0));
    let mut pipeline = ReceiverStreamPipeline::<FailingDecoder, 4, 8, 4>::new(
        &secret,
        context,
        FailingDecoder {
            fail_next_decode: true,
            reset_count: Arc::clone(&reset_count),
        },
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline.receive(&first.datagram, &mut plaintext).unwrap();

    assert_eq!(
        pipeline.decode_next(),
        Err(DecodeWorkerError::Decoder(FailingDecoderError))
    );
    assert_eq!(reset_count.load(Ordering::Relaxed), 1);
    assert_eq!(counters.snapshot().queued_pcm_blocks, 0);

    pipeline.receive(&second.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 1, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            discontinuity: true,
            discarded_stale_blocks: 0,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.authenticated_packets, 2);
    assert_eq!(snapshot.rejected_packets, 0);
    assert_eq!(snapshot.decode_errors, 1);
    assert_eq!(snapshot.stream_resets, 1);
}

#[test]
fn accepted_out_of_order_packet_increments_only_the_reordered_metric() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = sealer.seal(0, 0, &[0]).unwrap();
    let reordered = sealer.seal(0, 960, &[1]).unwrap();
    let future = sealer.seal(0, 1_920, &[2]).unwrap();
    let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut plaintext = [0_u8; 8];

    assert_eq!(
        pipeline.receive(&first.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        pipeline.receive(&future.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(
        pipeline
            .receive(&reordered.datagram, &mut plaintext)
            .unwrap(),
        InsertOutcome::Reordered
    );
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.reordered_packets, 1);
    assert_eq!(snapshot.late_packets, 0);
    assert_eq!(snapshot.rejected_packets, 0);
}
