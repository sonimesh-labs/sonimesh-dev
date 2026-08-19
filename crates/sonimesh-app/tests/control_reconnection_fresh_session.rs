//! Control-domain reconnection supplies a fresh media session (ADR 0017).
//!
//! The authenticated control-plane transport remains an external gate, but this
//! test pins the exact contract that transport must satisfy when the control
//! channel is lost while the UDP media path stays reachable: the coordinator
//! enters a bounded reconnect grace window and the media path keeps admitting
//! packets; the reconnected transport installs a fresh control session and
//! repeats hello negotiation; the pairing media root allocator supplies a fresh
//! media key context for the re-admitted stream; and the receiver pipeline
//! installs that session over the same socket, so packets sealed under the
//! retired session are rejected while the fresh session's packets rebind the
//! source and decode.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, MediaSessionAllocator,
    MediaSessionAllocatorError, MediaStreamIdentity, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingProofVerification, PairingSource,
    ReconnectGraceObservation, SessionEvent, SessionPhase, SessionState, UdpPollOutcome,
    UdpReceiverError, UdpReceiverPipeline,
};
use sonimesh_audio::{DecodeStep, FrameDecoder, RealtimePcmPopOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::UdpMediaSender;
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MediaDirection, MediaSealer,
    SessionSecret, StartStream, envelope,
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

const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;
const CAPABILITIES: &[Capability] = &[Capability::FrameDuration20Ms];

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

type Pipeline = UdpReceiverPipeline<ByteDecoder, 4, 8, 4>;

fn hello() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: CAPABILITIES
                .iter()
                .map(|capability| *capability as i32)
                .collect(),
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn start(session_id: u64, stream_id: u32, epoch: u32, request_id: u64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: session_id,
            stream_id,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: epoch,
        })),
    }
}

/// Builds a trusted READY coordinator through the public pairing owner, then
/// installs an authenticated control transport token.
fn ready_coordinator() -> (ControlStreamCoordinator, ControlSessionToken) {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut session = SessionState::IDLE;
    pairing
        .begin(&mut session, PairingId::new([1; 16]), 100, 50, 1)
        .expect("pairing invitation begins");
    assert_eq!(
        pairing.verify_proof(&mut session, 101, PairingSource::new([2; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );

    let mut coordinator = ControlStreamCoordinator::new(session, CAPABILITIES)
        .expect("coordinator with bounded local capabilities");
    let token = coordinator
        .install_authenticated_control_session()
        .expect("fresh transport token");
    (coordinator, token)
}

/// Negotiates hello and completes one start, returning the admitted request.
fn negotiate_and_start(
    coordinator: &mut ControlStreamCoordinator,
    token: ControlSessionToken,
    envelope: &Envelope,
) -> sonimesh_app::StreamStartRequest {
    assert!(matches!(
        coordinator.accept(token, &hello()),
        Ok(ControlStreamAction::Negotiated(_))
    ));
    let action = coordinator
        .accept(token, envelope)
        .expect("start envelope is accepted");
    let ControlStreamAction::Start { token: op, request } = action else {
        panic!("expected a start action, got {action:?}");
    };
    coordinator
        .complete_start(op, request)
        .expect("start completes");
    request
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
#[allow(clippy::too_many_lines)] // one end-to-end reconnection scenario
fn control_reconnection_supplies_a_fresh_session_while_udp_stays_reachable() {
    let counters = Arc::new(StreamCounters::default());
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    // First authenticated control session and media stream.
    let (mut coordinator, token) = ready_coordinator();
    let first_grant = allocator
        .supply(identity(7, 9, 0), DIRECTION)
        .expect("first identity is fresh");
    let first_context = first_grant.context();
    let first = negotiate_and_start(&mut coordinator, token, &start(7, 9, 0, 10));
    assert_eq!(first.identity, identity(7, 9, 0));
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);

    // The allocator's retained root backs the pipeline bind and the sender.
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).expect("pcm queue");
    let mut pipeline = Pipeline::bind(
        "127.0.0.1:0".parse().expect("loopback address"),
        first_grant.root(),
        first_context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .expect("pipeline with first session");
    let old_sender =
        UdpMediaSender::bind("127.0.0.1:0".parse().expect("loopback address")).expect("old sender");
    let mut old_sealer = MediaSealer::new(first_grant.root(), first_context).expect("old sealer");
    let first_packet = old_sealer.seal(0, 0, &[1]).expect("first packet");
    assert!(
        old_sender
            .send_to(
                pipeline.local_addr().expect("pipeline address"),
                &first_packet.datagram
            )
            .expect("first send")
    );
    assert!(matches!(
        poll_until_datagram(&mut pipeline).expect("first admission"),
        UdpPollOutcome::Processed { .. }
    ));
    assert_eq!(
        pipeline.pinned_source(),
        Some(old_sender.local_addr().unwrap())
    );
    assert_eq!(counters.snapshot().authenticated_packets, 1);

    // Control-channel loss while UDP remains reachable: the coordinator enters
    // a bounded reconnect window, and the still-installed media session keeps
    // admitting packets over the live socket.
    coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost)
        .expect("connection loss is valid from streaming");
    assert_eq!(coordinator.state().phase(), SessionPhase::Reconnecting);
    coordinator
        .begin_reconnect_grace(1_000, 30_000)
        .expect("grace arms");
    match coordinator.observe_reconnect_grace(5_000) {
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: 26_000,
        }) => {}
        other => panic!("expected an in-grace observation, got {other:?}"),
    }
    let in_grace_packet = old_sealer.seal(1, 0, &[2]).expect("in-grace packet");
    assert!(
        old_sender
            .send_to(pipeline.local_addr().unwrap(), &in_grace_packet.datagram)
            .expect("in-grace send")
    );
    assert!(matches!(
        poll_until_datagram(&mut pipeline).expect("in-grace admission"),
        UdpPollOutcome::Processed { .. }
    ));
    assert_eq!(counters.snapshot().authenticated_packets, 2);

    // Authenticated reconnection: the coordinator returns to Ready and clears
    // the retired transport, negotiation, and media identity; the transport
    // installs a fresh token and renegotiates.
    coordinator
        .acknowledge_platform_event(SessionEvent::Reconnected)
        .expect("reconnect is valid from reconnecting");
    assert_eq!(coordinator.state().phase(), SessionPhase::Ready);
    assert_eq!(coordinator.active(), None);

    let fresh_token = coordinator
        .install_authenticated_control_session()
        .expect("fresh transport token");
    let fresh_grant = allocator
        .supply(identity(8, 9, 0), DIRECTION)
        .expect("reconnection identity is fresh");
    let fresh_context = fresh_grant.context();
    let fresh = negotiate_and_start(&mut coordinator, fresh_token, &start(8, 9, 0, 20));
    assert_eq!(fresh.identity, identity(8, 9, 0));
    assert_eq!(coordinator.state().phase(), SessionPhase::Streaming);

    // The fresh session installs over the same socket and clears the source pin.
    let previous_addr = pipeline.local_addr().expect("pipeline address");
    pipeline
        .install_fresh_session(fresh_grant.root(), fresh_context, 0)
        .expect("fresh session installs");
    assert_eq!(pipeline.local_addr().unwrap(), previous_addr);
    assert_eq!(pipeline.pinned_source(), None);

    // Packets sealed under the retired session are rejected after the fresh
    // install and must not rebind the cleared source pin.
    let retired_packet = old_sealer.seal(2, 0, &[3]).expect("retired packet");
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
    let mut fresh_sealer =
        MediaSealer::new(fresh_grant.root(), fresh_context).expect("fresh sealer");
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
    assert_eq!(counters.snapshot().authenticated_packets, 3);
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

    // The allocator enforces the no-reuse record across the whole reconnection:
    // neither the retired nor the fresh key context can be issued again.
    assert_eq!(
        allocator
            .supply(identity(7, 9, 0), DIRECTION)
            .map(|grant| grant.context()),
        Err(MediaSessionAllocatorError::IdentityReused)
    );
    assert_eq!(
        allocator
            .supply(identity(8, 9, 0), DIRECTION)
            .map(|grant| grant.context()),
        Err(MediaSessionAllocatorError::IdentityReused)
    );
    assert_eq!(allocator.issued_count(), 2);
}
