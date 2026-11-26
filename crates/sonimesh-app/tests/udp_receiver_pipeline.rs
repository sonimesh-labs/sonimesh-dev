//! Loopback tests for the UDP-to-protected receiver composition.

use core::convert::Infallible;
use std::net::UdpSocket;
use std::sync::Arc;
use std::thread;

use sonimesh_app::{ReceiverPipelineError, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline};
use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, FrameError, RealtimePcmPopOutcome,
    StreamDecodeWorker, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{IngressError, UdpReceiveError};
use sonimesh_protocol::{
    MediaDirection, MediaKeyContext, MediaSealer, ReplayDecision, SessionSecret,
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

fn send(sender: &UdpSocket, pipeline: &Pipeline, datagram: &[u8]) {
    sender
        .send_to(datagram, pipeline.local_addr().unwrap())
        .unwrap();
}

fn poll_until_datagram(
    pipeline: &mut Pipeline,
) -> Result<UdpPollOutcome, UdpReceiverError<Infallible>> {
    for _ in 0..1_000 {
        match pipeline.poll() {
            Ok(UdpPollOutcome::Empty) => thread::yield_now(),
            result => return result,
        }
    }
    panic!("loopback datagram was not delivered");
}

#[test]
fn reserved_socket_supports_successive_sessions_without_trusting_old_packets() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let old_context = context();
    let new_context = MediaKeyContext::new(42, 9, 1, MediaDirection::AndroidToDesktop).unwrap();
    let mut old_sealer = MediaSealer::new(&secret, old_context).unwrap();
    let old_packet = old_sealer.seal(0, 0, &[7]).unwrap();
    let make_receiver = |context| {
        let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
        Pipeline::from_socket_with_worker(
            socket.try_clone().unwrap(),
            &secret,
            context,
            StreamDecodeWorker::new(ByteDecoder, producer, 0),
            Arc::new(StreamCounters::default()),
        )
        .unwrap()
    };
    let mut first = make_receiver(old_context);
    send(&sender, &first, &old_packet.datagram);
    assert!(matches!(
        poll_until_datagram(&mut first),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    drop(first);
    sender
        .send_to(&old_packet.datagram, socket.local_addr().unwrap())
        .unwrap();
    let mut second = make_receiver(new_context);
    assert!(poll_until_datagram(&mut second).is_err());
    assert_eq!(second.pinned_source(), None);
    let mut new_sealer = MediaSealer::new(&secret, new_context).unwrap();
    let new_packet = new_sealer.seal(0, 0, &[8]).unwrap();
    send(&sender, &second, &new_packet.datagram);
    assert!(matches!(
        poll_until_datagram(&mut second),
        Ok(UdpPollOutcome::Processed { .. })
    ));
}

#[test]
fn receives_authenticated_udp_media_before_decode() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    assert_eq!(pipeline.pinned_source(), None);
    send(&sender, &pipeline, &packet.datagram);

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
fn authenticated_empty_payload_is_rejected_before_decoder_and_does_not_pin_source() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let mut sealer = MediaSealer::new(&secret, context()).unwrap();
    let empty = sealer.seal(0, 0, &[]).unwrap();

    send(&sender, &pipeline, &empty.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Decode(
            DecodeWorkerError::Frame(FrameError::EmptyPayload)
        )))
    ));
    assert_eq!(pipeline.pinned_source(), None);
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow { .. }
    ));
    assert!(output.iter().all(|sample| sample.abs() < f32::EPSILON));

    // The authenticated packet still consumed its sequence, so a replay is
    // rejected as a duplicate instead of retrying decoder admission.
    send(&sender, &pipeline, &empty.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
            IngressError::Replay(ReplayDecision::Duplicate)
        )))
    ));
    assert_eq!(pipeline.pinned_source(), None);

    // A valid packet from the same sender establishes the pin normally. The
    // rejected empty frame still occupies its timeline slot, so playout first
    // conceals the missing sequence before decoding the valid frame.
    let valid = sealer.seal(0, 960, &[7]).unwrap();
    send(&sender, &pipeline, &valid.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    assert_eq!(pipeline.pinned_source(), Some(sender.local_addr().unwrap()));
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Concealed { sequence: 0, .. }
    ));
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded {
            sequence: 1,
            samples: 1,
            ..
        }
    ));

    let snapshot = counters.snapshot();
    assert_eq!(snapshot.authenticated_packets, 3);
    assert_eq!(snapshot.rejected_packets, 2);
    assert_eq!(snapshot.duplicate_packets, 1);
}

#[test]
fn unauthenticated_packet_cannot_pin_the_source_endpoint() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(Arc::clone(&counters));
    let unauthenticated_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    send(&unauthenticated_sender, &pipeline, &[0]);

    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::Pipeline(_))
    ));
    assert_eq!(pipeline.pinned_source(), None);

    let authenticated_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    send(&authenticated_sender, &pipeline, &packet.datagram);

    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(authenticated_sender.local_addr().unwrap())
    );
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.authenticated_packets, 1);
    assert_eq!(snapshot.rejected_packets, 1);
}

#[test]
fn changed_source_is_rejected_without_advancing_protected_ingress() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(Arc::clone(&counters));
    let authenticated_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let changed_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&SessionSecret::new([0xA5; 32]), context()).unwrap();
    let first = sealer.seal(0, 0, &[7]).unwrap();
    let second = sealer.seal(0, 960, &[8]).unwrap();

    send(&authenticated_sender, &pipeline, &first.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));

    send(&changed_sender, &pipeline, &second.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::SourceEndpointChanged)
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(authenticated_sender.local_addr().unwrap())
    );

    send(&authenticated_sender, &pipeline, &second.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.authenticated_packets, 2);
    assert_eq!(snapshot.rejected_packets, 1);
}

#[test]
fn key_epoch_rotation_does_not_allow_source_migration() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(Arc::clone(&counters));
    let authenticated_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let changed_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let mut initial_sealer = MediaSealer::new(&secret, context()).unwrap();
    let first = initial_sealer.seal(0, 0, &[7]).unwrap();
    send(&authenticated_sender, &pipeline, &first.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));

    let previous_context = context();
    let next_context = MediaKeyContext::new(
        previous_context.session_id,
        previous_context.stream_id,
        previous_context.key_epoch + 1,
        previous_context.direction,
    )
    .unwrap();
    pipeline.rotate(&secret, next_context, 0).unwrap();
    let mut rotated_sealer = MediaSealer::new(&secret, next_context).unwrap();
    let rotated = rotated_sealer.seal(0, 0, &[8]).unwrap();

    send(&changed_sender, &pipeline, &rotated.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::SourceEndpointChanged)
    ));
    send(&authenticated_sender, &pipeline, &rotated.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
}

#[test]
fn failed_fresh_session_install_preserves_the_source_binding() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, _consumer) = pipeline(counters);
    let authenticated_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let changed_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let secret = SessionSecret::new([0xA5; 32]);
    let mut sealer = MediaSealer::new(&secret, context()).unwrap();
    let first = sealer.seal(0, 0, &[7]).unwrap();
    let second = sealer.seal(0, 960, &[8]).unwrap();
    send(&authenticated_sender, &pipeline, &first.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));

    assert!(
        pipeline
            .install_fresh_session(&SessionSecret::new([0x5A; 32]), context(), 0)
            .is_err()
    );
    assert_eq!(
        pipeline.pinned_source(),
        Some(authenticated_sender.local_addr().unwrap())
    );

    send(&changed_sender, &pipeline, &second.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::SourceEndpointChanged)
    ));
    send(&authenticated_sender, &pipeline, &second.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
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
fn fresh_session_keeps_the_socket_and_allows_new_source_rebinding() {
    let counters = Arc::new(StreamCounters::default());
    let (mut pipeline, mut consumer) = pipeline(Arc::clone(&counters));
    let previous_addr = pipeline.local_addr().unwrap();
    let previous_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let previous_secret = SessionSecret::new([0xA5; 32]);
    let previous_context = context();
    let mut previous_sealer = MediaSealer::new(&previous_secret, previous_context).unwrap();
    let previous_packet = previous_sealer.seal(0, 0, &[7]).unwrap();
    send(&previous_sender, &pipeline, &previous_packet.datagram);
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(previous_sender.local_addr().unwrap())
    );

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
    assert_eq!(pipeline.pinned_source(), None);

    let fresh_sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut sealer = MediaSealer::new(&fresh_secret, fresh_context).unwrap();
    let packet = sealer.seal(0, 0, &[8]).unwrap();
    send(&fresh_sender, &pipeline, &packet.datagram);

    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Ok(UdpPollOutcome::Processed { .. })
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(fresh_sender.local_addr().unwrap())
    );
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
}
