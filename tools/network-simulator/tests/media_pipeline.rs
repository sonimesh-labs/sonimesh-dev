//! Cross-crate deterministic media-pipeline regression test.

use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmConsumer, RealtimePcmPopOutcome,
    RealtimePcmPushOutcome, StreamDecodeWorker, realtime_pcm_queue,
};
use sonimesh_network::{IngressError, ProtectedMediaIngress};
use sonimesh_network_simulator::{NetworkProfile, SendOutcome, VirtualNetwork};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaKeyContext,
    MediaSealer, SessionSecret, encode_control_frame, envelope,
};

const FRAMES: u32 = 200;
const FRAME_MS: u64 = 20;
const TARGET_MS: u64 = 60;
const SESSION_ID: u64 = 7;
const STREAM_ID: u32 = 9;
const KEY_EPOCH: u16 = 1;

struct LabDecoder;

impl FrameDecoder for LabDecoder {
    type Error = core::convert::Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        assert_eq!(encoded.len(), 4);
        output[0] = 1.0;
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }
}

struct SequenceDecoder;

impl FrameDecoder for SequenceDecoder {
    type Error = core::convert::Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        assert_eq!(encoded.len(), 4);
        output[0] = f32::from(encoded[3]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }
}

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        SESSION_ID,
        STREAM_ID,
        KEY_EPOCH,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

fn media_datagram(sealer: &mut MediaSealer, sequence: u32) -> Vec<u8> {
    let protected = sealer
        .seal(0, u64::from(sequence) * 960, &sequence.to_be_bytes())
        .unwrap();
    assert_eq!(protected.header.sequence, sequence);
    protected.datagram
}

fn control_frame() -> Vec<u8> {
    encode_control_frame(&Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "simulated-control".to_owned(),
        })),
    })
    .unwrap()
}

type Worker = StreamDecodeWorker<LabDecoder, 16, 8, 1>;

fn decode_and_render(
    worker: &mut Worker,
    consumer: &mut RealtimePcmConsumer<1>,
    sequence: u32,
) -> bool {
    let concealed = match worker.decode_next().unwrap() {
        DecodeStep::Decoded {
            sequence: actual,
            samples,
            queue,
        } => {
            assert_eq!(actual, sequence);
            assert_eq!(samples, 1);
            assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
            false
        }
        DecodeStep::Concealed {
            sequence: actual,
            samples,
            queue,
        } => {
            assert_eq!(actual, sequence);
            assert_eq!(samples, 1);
            assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
            true
        }
        DecodeStep::NotReady => panic!("negotiated playout must produce a decision"),
    };
    let mut output = [f32::NAN];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: u64::from(sequence),
            discontinuity: false,
            discarded_stale_blocks: 0,
        }
    );
    assert!(output[0].is_finite());
    concealed
}

#[test]
fn authenticated_pipeline_stays_bounded_under_impairment() {
    let mut profile = NetworkProfile::stable(0xC0DE_2026);
    profile.loss_percent = 3;
    profile.duplicate_percent = 20;
    profile.reorder_percent = 20;
    profile.jitter_ms = 10;
    profile.reorder_delay_ms = 25;
    let mut network = VirtualNetwork::new(profile).unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let (pcm_producer, mut pcm_consumer) = realtime_pcm_queue::<1>(4).unwrap();
    let mut worker = StreamDecodeWorker::<LabDecoder, 16, 8, 1>::new(LabDecoder, pcm_producer, 0);
    let mut plaintext = [0_u8; 8];
    let mut next_send = 0_u32;
    let mut next_playout = 0_u32;
    let mut replayed = 0_u32;
    let mut rejected = 0_u32;
    let mut plc_frames = 0_u32;
    let mut rendered = 0_u32;
    let mut jitter_resets = 0_u32;
    let end_ms = TARGET_MS + u64::from(FRAMES - 1) * FRAME_MS + 100;

    for now_ms in 0..=end_ms {
        if next_send < FRAMES && now_ms == u64::from(next_send) * FRAME_MS {
            let datagram = media_datagram(&mut sealer, next_send);
            let payload_bytes = u64::try_from(datagram.len()).unwrap_or(u64::MAX);
            network.send(now_ms, datagram, payload_bytes);
            next_send += 1;
        }

        for delivery in network.drain_ready(now_ms) {
            match ingress.receive(&delivery.payload, &mut plaintext) {
                Ok(frame) => {
                    match worker
                        .admit(
                            frame.header.sequence,
                            frame.header.sample_timestamp,
                            frame.opus_payload,
                        )
                        .unwrap()
                    {
                        InsertOutcome::Inserted | InsertOutcome::Reordered => {}
                        InsertOutcome::Resynchronized { .. } => {
                            worker.reset_after_jitter_resynchronization();
                            jitter_resets += 1;
                        }
                        InsertOutcome::Duplicate | InsertOutcome::Late => rejected += 1,
                        InsertOutcome::SequenceExhausted => {
                            panic!("test sequence range must not exhaust")
                        }
                        InsertOutcome::Disabled => panic!("test jitter buffer must be enabled"),
                    }
                }
                Err(IngressError::Replay(_)) => replayed += 1,
                Err(_) => rejected += 1,
            }
        }

        if next_playout < FRAMES && now_ms == TARGET_MS + u64::from(next_playout) * FRAME_MS {
            plc_frames += u32::from(decode_and_render(
                &mut worker,
                &mut pcm_consumer,
                next_playout,
            ));
            rendered += 1;
            next_playout += 1;
        }
    }

    assert_eq!(
        (next_send, rendered, rejected, jitter_resets),
        (FRAMES, FRAMES, 0, 0)
    );
    assert!(
        plc_frames > 0
            && replayed > 0
            && network.stats().random_loss_packets > 0
            && network.queued_copies() <= profile.max_in_flight_packets
    );
}

#[test]
fn control_loss_does_not_block_an_independent_authenticated_media_path() {
    let mut control_profile = NetworkProfile::stable(0x0C01_1055);
    control_profile.loss_percent = 100;
    control_profile.base_delay_ms = 0;
    control_profile.jitter_ms = 0;
    let mut control_network = VirtualNetwork::new(control_profile).unwrap();
    let control = control_frame();
    assert_eq!(
        control_network.send(0, control.clone(), u64::try_from(control.len()).unwrap()),
        SendOutcome::DroppedByLoss
    );
    assert!(control_network.drain_ready(0).is_empty());
    assert_eq!(control_network.stats().random_loss_packets, 1);

    let mut media_profile = NetworkProfile::stable(0x0DA7_A055);
    media_profile.base_delay_ms = 0;
    media_profile.jitter_ms = 0;
    let mut media_network = VirtualNetwork::new(media_profile).unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<1>(4).unwrap();
    let mut worker = StreamDecodeWorker::<LabDecoder, 16, 8, 1>::new(LabDecoder, producer, 0);
    let mut plaintext = [0_u8; 8];
    let datagram = media_datagram(&mut sealer, 0);
    let datagram_bytes = u64::try_from(datagram.len()).unwrap();
    assert_eq!(
        media_network.send(0, datagram, datagram_bytes),
        SendOutcome::Scheduled { copies: 1 }
    );

    let delivery = media_network.drain_ready(0).pop().unwrap();
    let frame = ingress.receive(&delivery.payload, &mut plaintext).unwrap();
    assert_eq!(
        worker
            .admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
            .unwrap(),
        InsertOutcome::Inserted
    );
    assert!(!decode_and_render(&mut worker, &mut consumer, 0));
    assert_eq!(media_network.stats().delivered_copies, 1);
}

#[test]
fn decode_worker_generates_plc_before_the_callback_and_bounds_a_stalled_pcm_queue() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = media_datagram(&mut sealer, 0);
    let _missing = media_datagram(&mut sealer, 1);
    let third = media_datagram(&mut sealer, 2);
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<1>(2).unwrap();
    let mut worker =
        StreamDecodeWorker::<SequenceDecoder, 4, 8, 1>::new(SequenceDecoder, producer, 0);
    let mut plaintext = [0_u8; 8];

    for datagram in [&first, &third] {
        let frame = ingress.receive(datagram, &mut plaintext).unwrap();
        assert_eq!(
            worker
                .admit(
                    frame.header.sequence,
                    frame.header.sample_timestamp,
                    frame.opus_payload,
                )
                .unwrap(),
            InsertOutcome::Inserted
        );
        plaintext.fill(0);
    }

    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 0,
            samples: 1,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Concealed {
            sequence: 1,
            samples: 1,
            queue: RealtimePcmPushOutcome::Inserted,
        }
    );
    assert_eq!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 2,
            samples: 1,
            queue: RealtimePcmPushOutcome::ReplacedOldest,
        }
    );
    assert_eq!(worker.queued_blocks(), 2);

    let mut output = [f32::NAN];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 1,
            discontinuity: true,
            discarded_stale_blocks: 0,
        }
    );
    assert!(output[0].abs() <= f32::EPSILON);

    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 2,
            discontinuity: false,
            discarded_stale_blocks: 0,
        }
    );
    assert!((output[0] - 2.0).abs() <= f32::EPSILON);
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 0,
        }
    );
    assert!(output[0].abs() <= f32::EPSILON);
}

#[test]
fn authenticated_jitter_resync_retires_old_pcm_and_keeps_the_resync_packet() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let first = media_datagram(&mut sealer, 0);
    let buffered = media_datagram(&mut sealer, 1);
    let _skipped = media_datagram(&mut sealer, 2);
    let resync_point = media_datagram(&mut sealer, 3);
    let mut ingress = ProtectedMediaIngress::new(&secret, context).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<1>(4).unwrap();
    let mut worker =
        StreamDecodeWorker::<SequenceDecoder, 2, 8, 1>::new(SequenceDecoder, producer, 0);
    let mut plaintext = [0_u8; 8];

    let frame = ingress.receive(&first, &mut plaintext).unwrap();
    assert_eq!(
        worker
            .admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
            .unwrap(),
        InsertOutcome::Inserted
    );
    plaintext.fill(0);
    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));

    let frame = ingress.receive(&buffered, &mut plaintext).unwrap();
    assert_eq!(
        worker
            .admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
            .unwrap(),
        InsertOutcome::Inserted
    );
    plaintext.fill(0);
    let frame = ingress.receive(&resync_point, &mut plaintext).unwrap();
    assert_eq!(
        worker
            .admit(
                frame.header.sequence,
                frame.header.sample_timestamp,
                frame.opus_payload,
            )
            .unwrap(),
        InsertOutcome::Resynchronized { dropped_frames: 1 }
    );
    plaintext.fill(0);
    worker.reset_after_jitter_resynchronization();

    assert!(matches!(
        worker.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 3, .. }
    ));
    let mut output = [f32::NAN];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 0,
            discontinuity: true,
            discarded_stale_blocks: 1,
        }
    );
    assert!((output[0] - 3.0).abs() < f32::EPSILON);
}
