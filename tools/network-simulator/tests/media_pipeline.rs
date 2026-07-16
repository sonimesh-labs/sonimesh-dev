//! Cross-crate deterministic media-pipeline regression test.

use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmConsumer, RealtimePcmPopOutcome,
    RealtimePcmPushOutcome, StreamDecodeWorker, realtime_pcm_queue,
};
use sonimesh_network::{IngressError, MediaIngress, ProtocolAuthenticator, StreamIdentity};
use sonimesh_network_simulator::{NetworkProfile, VirtualNetwork};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaOpener, MediaSealer, SessionSecret};

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
    let opener = MediaOpener::new(&secret, context).unwrap();
    let mut ingress = MediaIngress::new(
        StreamIdentity {
            session_id: SESSION_ID,
            stream_id: STREAM_ID,
            key_epoch: KEY_EPOCH,
        },
        ProtocolAuthenticator::new(opener),
    );
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
            network.send(now_ms, media_datagram(&mut sealer, next_send));
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
                        InsertOutcome::Inserted => {}
                        InsertOutcome::Resynchronized { .. } => jitter_resets += 1,
                        InsertOutcome::Duplicate | InsertOutcome::Late => rejected += 1,
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
