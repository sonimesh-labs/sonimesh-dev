//! Loopback tests for the UDP-to-protected receiver composition.

use core::convert::Infallible;
use std::net::UdpSocket;
use std::sync::Arc;
use std::thread;

use sonimesh_app::{UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline};
use sonimesh_audio::{DecodeStep, FrameDecoder, RealtimePcmPopOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::UdpReceiveError;
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

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

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        1,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

fn pipeline(counters: Arc<StreamCounters>) -> (Pipeline, sonimesh_audio::RealtimePcmConsumer<4>) {
    let secret = SessionSecret::new([0xA5; 32]);
    let (producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context(),
        ByteDecoder,
        producer,
        counters,
        0,
    )
    .unwrap();
    (pipeline, consumer)
}

#[test]
fn receives_authenticated_udp_media_before_decode() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    sender
        .send_to(&packet.datagram, pipeline.local_addr().unwrap())
        .unwrap();

    for _ in 0..1_000 {
        match pipeline.poll().unwrap() {
            UdpPollOutcome::Empty => thread::yield_now(),
            UdpPollOutcome::Processed { admission, .. } => {
                assert_eq!(admission, sonimesh_audio::InsertOutcome::Inserted);
                assert!(matches!(
                    pipeline.decode_next().unwrap(),
                    DecodeStep::Decoded { .. }
                ));
                let mut output = [f32::NAN; 4];
                assert!(matches!(
                    consumer.pop_into(&mut output),
                    RealtimePcmPopOutcome::Filled { samples: 1, .. }
                ));
                assert!((output[0] - 7.0).abs() < f32::EPSILON);
                assert_eq!(counters.snapshot().authenticated_packets, 1);
                return;
            }
        }
    }
    panic!("loopback media datagram was not processed");
}

#[test]
fn oversized_udp_media_is_rejected_and_counted_before_pipeline_ingress() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let payload = [0xA5; sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN + 1];
    sender
        .send_to(&payload, pipeline.local_addr().unwrap())
        .unwrap();

    for _ in 0..1_000 {
        match pipeline.poll() {
            Err(UdpReceiverError::Socket(UdpReceiveError::DatagramTooLarge)) => {
                assert_eq!(counters.snapshot().rejected_packets, 1);
                return;
            }
            Ok(UdpPollOutcome::Empty) => thread::yield_now(),
            other => panic!("unexpected UDP pipeline result: {other:?}"),
        }
    }
    panic!("oversized loopback datagram was not rejected");
}

#[test]
fn udp_pipeline_keeps_its_socket_while_installing_a_fresh_media_session() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let previous_addr = pipeline.local_addr().unwrap();
    let previous_context = context();
    let fresh_context = MediaKeyContext::new(
        previous_context.session_id + 1,
        previous_context.stream_id,
        0,
        previous_context.direction,
    )
    .unwrap();
    let fresh_secret = SessionSecret::new([0x5A; 32]);
    pipeline
        .install_fresh_session(&fresh_secret, fresh_context, 0)
        .unwrap();
    assert_eq!(pipeline.local_addr().unwrap(), previous_addr);

    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&fresh_secret, fresh_context).unwrap();
    let packet = sealer.seal(0, 0, &[8]).unwrap();
    sender.send_to(&packet.datagram, previous_addr).unwrap();

    for _ in 0..1_000 {
        match pipeline.poll().unwrap() {
            UdpPollOutcome::Empty => thread::yield_now(),
            UdpPollOutcome::Processed { .. } => {
                assert!(matches!(
                    pipeline.decode_next().unwrap(),
                    DecodeStep::Decoded { .. }
                ));
                let mut output = [f32::NAN; 4];
                assert!(matches!(
                    consumer.pop_into(&mut output),
                    RealtimePcmPopOutcome::Filled { .. }
                ));
                assert!((output[0] - 8.0).abs() < f32::EPSILON);
                assert_eq!(counters.snapshot().stream_resets, 1);
                return;
            }
        }
    }
    panic!("fresh-session loopback media datagram was not processed");
}
