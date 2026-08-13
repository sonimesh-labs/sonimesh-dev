//! Loopback tests proving the bounded UDP sender feeds the receiver pipeline.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline};
use sonimesh_audio::{DecodeStep, FrameDecoder, RealtimePcmPopOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{UdpMediaSender, UdpSendError};
use sonimesh_protocol::{
    MAX_MEDIA_DATAGRAM_LEN, MediaDirection, MediaKeyContext, MediaSealer, SessionSecret,
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

fn poll_until_datagram(
    pipeline: &mut Pipeline,
) -> Result<UdpPollOutcome, UdpReceiverError<Infallible>> {
    for _ in 0..1_000 {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => std::thread::yield_now(),
            result => return result,
        }
    }
    panic!("loopback datagram was not delivered");
}

#[test]
fn bounded_sender_delivers_authenticated_media_through_the_pipeline() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpMediaSender::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();

    assert!(
        sender
            .send_to(pipeline.local_addr().unwrap(), &packet.datagram)
            .unwrap()
    );

    let UdpPollOutcome::Processed { source, admission } =
        poll_until_datagram(&mut pipeline).unwrap()
    else {
        panic!("loopback media datagram was not processed");
    };
    assert_eq!(source, sender.local_addr().unwrap());
    assert_eq!(pipeline.pinned_source(), Some(source));
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
}

#[test]
fn sender_rejects_an_oversized_datagram_before_the_wire() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpMediaSender::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let oversized = [0xA5; MAX_MEDIA_DATAGRAM_LEN + 1];

    assert!(matches!(
        sender.send_to(pipeline.local_addr().unwrap(), &oversized),
        Err(UdpSendError::DatagramTooLarge)
    ));
    assert_eq!(pipeline.pinned_source(), None);
    assert_eq!(pipeline.poll().unwrap(), UdpPollOutcome::Empty);
    assert_eq!(counters.snapshot().rejected_packets, 0);
}
