//! Loopback evidence for the ADR 0017 allocator feeding the UDP receiver.
//!
//! The allocator is the pairing/control-session owner of the one-time pairing
//! media root. This test drives the real loopback socket path with that root
//! and the contexts the allocator supplies: a first authenticated session
//! admits its own packets and pins the source; an authenticated reconnection
//! supplies a fresh session through the allocator;
//! `UdpReceiverPipeline::install_fresh_session` keeps the socket, clears the
//! pin, and rejects packets sealed under the retired session while the fresh
//! session's packets rebind the source and decode.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    MediaSessionAllocator, MediaStreamIdentity, UdpPollOutcome, UdpReceiverError,
    UdpReceiverPipeline,
};
use sonimesh_audio::{DecodeStep, FrameDecoder, RealtimePcmPopOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::UdpMediaSender;
use sonimesh_protocol::{MediaDirection, MediaSealer, SessionSecret};

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

const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

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
fn allocator_supplies_the_fresh_root_and_context_through_the_udp_path() {
    let counters = Arc::new(StreamCounters::default());
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    // First authenticated session: the allocator supplies the identity/context
    // and its retained root backs the sealer and the pipeline bind.
    let first_context = allocator
        .supply(identity(7, 1, 0), DIRECTION)
        .expect("first identity is fresh");
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).expect("pcm queue");
    let mut pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().expect("loopback address"),
        allocator.root(),
        first_context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .expect("pipeline with first session");

    let old_sender =
        UdpMediaSender::bind("127.0.0.1:0".parse().expect("loopback address")).expect("old sender");
    let mut old_sealer = MediaSealer::new(allocator.root(), first_context).expect("old sealer");
    let old_packet = old_sealer.seal(0, 0, &[1]).expect("old packet");
    assert!(
        old_sender
            .send_to(
                pipeline.local_addr().expect("pipeline address"),
                &old_packet.datagram
            )
            .expect("old send")
    );
    assert!(matches!(
        poll_until_datagram(&mut pipeline).expect("old admission"),
        UdpPollOutcome::Processed { .. }
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(old_sender.local_addr().unwrap())
    );
    assert_eq!(counters.snapshot().authenticated_packets, 1);

    // Authenticated reconnection supplies a fresh session through the same
    // pairing root: a new media session id is a fresh context (ADR 0017).
    let fresh_context = allocator
        .supply(identity(8, 1, 0), DIRECTION)
        .expect("reconnection session is fresh");
    let previous_addr = pipeline.local_addr().expect("pipeline address");
    pipeline
        .install_fresh_session(allocator.root(), fresh_context, 0)
        .expect("fresh session installs");
    assert_eq!(pipeline.local_addr().unwrap(), previous_addr);
    assert_eq!(pipeline.pinned_source(), None);

    // Packets sealed under the retired session are rejected after the fresh
    // install and must not rebind the cleared source pin.
    let retired_packet = old_sealer.seal(1, 0, &[2]).expect("retired packet");
    assert!(
        old_sender
            .send_to(pipeline.local_addr().unwrap(), &retired_packet.datagram)
            .expect("retired send")
    );
    assert!(matches!(
        poll_until_datagram(&mut pipeline),
        Err(UdpReceiverError::Pipeline(_))
    ));
    assert_eq!(pipeline.pinned_source(), None);
    assert_eq!(counters.snapshot().rejected_packets, 1);

    // The fresh session's packets rebind the source and decode.
    let fresh_sender = UdpMediaSender::bind("127.0.0.1:0".parse().expect("loopback address"))
        .expect("fresh sender");
    let mut fresh_sealer = MediaSealer::new(allocator.root(), fresh_context).expect("fresh sealer");
    let fresh_packet = fresh_sealer.seal(0, 0, &[5]).expect("fresh packet");
    assert!(
        fresh_sender
            .send_to(pipeline.local_addr().unwrap(), &fresh_packet.datagram)
            .expect("fresh send")
    );
    assert!(matches!(
        poll_until_datagram(&mut pipeline).expect("fresh admission"),
        UdpPollOutcome::Processed { .. }
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(fresh_sender.local_addr().unwrap())
    );
    assert_eq!(counters.snapshot().authenticated_packets, 2);
    assert!(matches!(
        pipeline.decode_next().expect("decode fresh packet"),
        DecodeStep::Decoded { .. }
    ));
    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled { samples: 1, .. }
    ));
    assert!((output[0] - 5.0).abs() < f32::EPSILON);
}
