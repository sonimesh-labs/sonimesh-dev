//! Loopback tests proving the bounded media session sender feeds the receiver
//! pipeline over real UDP sockets, including key-epoch rotation.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmPopOutcome, RealtimePcmPushOutcome,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{MediaSessionSendOutcome, MediaSessionSender, UdpMediaSender};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const SESSION_ID: u64 = 0x0102_0304_0506_0708;
const STREAM_ID: u32 = 9;
const FRAMES: u32 = 24;

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

fn context(key_epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        SESSION_ID,
        STREAM_ID,
        key_epoch,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

fn secret(byte: u8) -> SessionSecret {
    SessionSecret::new([byte; 32])
}

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

fn pipeline(counters: Arc<StreamCounters>) -> (Pipeline, sonimesh_audio::RealtimePcmConsumer<4>) {
    let (producer, consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(0xA5),
        context(1),
        ByteDecoder,
        producer,
        counters,
        0,
    )
    .unwrap();
    (pipeline, consumer)
}

fn poll_until_processed(
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

/// Sends one frame, admits exactly one datagram, and renders it in order.
fn send_admit_render(
    sender: &mut MediaSessionSender,
    pipeline: &mut Pipeline,
    consumer: &mut sonimesh_audio::RealtimePcmConsumer<4>,
    frame: u32,
) {
    let outcome = sender
        .send_frame(0, u64::from(frame) * 960, &[u8::try_from(frame).expect("test frame fits one byte")])
        .unwrap();
    assert_eq!(outcome, MediaSessionSendOutcome::Sent { sequence: frame });

    let UdpPollOutcome::Processed { admission, .. } = poll_until_processed(pipeline).unwrap()
    else {
        panic!("loopback media frame {frame} was not processed");
    };
    assert_eq!(admission, InsertOutcome::Inserted);

    match pipeline.decode_next().unwrap() {
        DecodeStep::Decoded {
            sequence,
            samples,
            queue,
        } => {
            assert_eq!(sequence, frame);
            assert_eq!(samples, 1);
            assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
        }
        other => panic!("loopback frame {frame} must decode, found {other:?}"),
    }

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 1, .. }
    ));
    assert!((output[0] - f32::from(u8::try_from(frame).expect("test frame fits one byte"))).abs() < f32::EPSILON);
}

#[test]
fn session_sender_streams_authenticated_frames_in_order() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let mut sender = MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(0xA5),
        context(1),
        pipeline.local_addr().unwrap(),
    )
    .unwrap();

    for frame in 0..FRAMES {
        send_admit_render(&mut sender, &mut pipeline, &mut consumer, frame);
    }

    assert_eq!(counters.snapshot().authenticated_packets, u64::from(FRAMES));
    assert_eq!(counters.snapshot().rejected_packets, 0);
}

#[test]
fn epoch_rotation_reaches_the_receiver_and_the_stream_keeps_flowing() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let mut sender = MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(0xA5),
        context(1),
        pipeline.local_addr().unwrap(),
    )
    .unwrap();

    send_admit_render(&mut sender, &mut pipeline, &mut consumer, 0);

    // Rotate both sides to epoch 2 with a fresh secret and a fresh sequence.
    sender.rotate(&secret(0xB5), context(2)).unwrap();
    pipeline.rotate(&secret(0xB5), context(2), 0).unwrap();

    // The first epoch-2 frame restarts at sequence zero.
    let outcome = sender.send_frame(0, 0, &[0x2E]).unwrap();
    assert_eq!(outcome, MediaSessionSendOutcome::Sent { sequence: 0 });
    let UdpPollOutcome::Processed { admission, .. } = poll_until_processed(&mut pipeline).unwrap()
    else {
        panic!("rotated loopback frame was not processed");
    };
    assert_eq!(admission, InsertOutcome::Inserted);
    match pipeline.decode_next().unwrap() {
        DecodeStep::Decoded {
            sequence,
            samples,
            queue,
        } => {
            assert_eq!(sequence, 0);
            assert_eq!(samples, 1);
            assert_eq!(queue, RealtimePcmPushOutcome::Inserted);
        }
        other => panic!("rotated frame must decode, found {other:?}"),
    }
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 1, .. }
    ));
    assert!((output[0] - f32::from(0x2E_u8)).abs() < f32::EPSILON);

    // A stale epoch-1 datagram from a different source is rejected by the pin
    // and counted, and one bad datagram must not break the stream.
    let mut stale = MediaSealer::new(&secret(0xA5), context(1)).unwrap();
    let stale_datagram = stale.seal(0, 0, &[0x11]).unwrap().datagram;
    let impostor = UdpMediaSender::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    assert!(
        impostor
            .send_to(pipeline.local_addr().unwrap(), &stale_datagram)
            .unwrap()
    );
    assert!(matches!(
        poll_until_processed(&mut pipeline),
        Err(UdpReceiverError::SourceEndpointChanged)
    ));

    send_admit_render(&mut sender, &mut pipeline, &mut consumer, 1);

    assert!(counters.snapshot().rejected_packets >= 1);
    assert_eq!(
        counters.snapshot().authenticated_packets,
        1 + 1 + 1, // epoch-1 frame 0, epoch-2 frame 0, epoch-2 frame 1
    );
}

#[test]
fn fresh_session_rebinds_the_same_source_after_the_pin_is_cleared() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let mut sender = MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &secret(0xA5),
        context(1),
        pipeline.local_addr().unwrap(),
    )
    .unwrap();

    send_admit_render(&mut sender, &mut pipeline, &mut consumer, 0);
    assert!(pipeline.pinned_source().is_some());

    // A fresh session changes session and stream identity; both sides install
    // it and the receiver clears the source pin for the fresh session.
    let fresh_context = MediaKeyContext::new(
        0x1122_3344_5566_7788,
        21,
        1,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap();
    sender
        .install_fresh_session(&secret(0xC5), fresh_context)
        .unwrap();
    pipeline
        .install_fresh_session(&secret(0xC5), fresh_context, 0)
        .unwrap();
    assert_eq!(pipeline.pinned_source(), None);

    // The same source socket is admitted again after the fresh session.
    send_admit_render(&mut sender, &mut pipeline, &mut consumer, 0);
    assert!(pipeline.pinned_source().is_some());
    assert_eq!(counters.snapshot().authenticated_packets, 2);
}

