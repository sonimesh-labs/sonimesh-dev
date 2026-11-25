//! Software-only integration of the native lab decoder and desktop scheduler.

#[path = "../src/bin/lab_decoder/mod.rs"]
mod lab_decoder;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonimesh_app::{
    ControlSessionPoll, ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner,
    DesktopReceiverScheduler, OutputSelection, PairingAttemptOutcome, PairingId,
    PairingInvitationController, PairingProofVerification, PairingSource, ScheduledReceiverState,
    SessionState, UdpReceiverPipeline,
};
use sonimesh_audio::{DecodeStep, OpusFrameConfig, StreamDecodeWorker, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::MediaSessionSender;
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, Envelope, Hello, MAX_CONTROL_FRAME_BYTES, MediaDirection,
    MediaKeyContext, SessionSecret, StartStream, StopStream, StreamState, decode_control_frame,
    encode_control_frame, envelope,
};

const SAMPLES: usize = 1_920;

#[test]
fn native_ten_ms_decode_and_plc_fit_a_larger_reserved_block() {
    let mut encoder =
        opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
    let mut packet = [0_u8; 1_148];
    let length = encoder.encode_float(&[0.1; 960], &mut packet).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<SAMPLES>(4).unwrap();
    let mut worker = StreamDecodeWorker::<_, 4, 1_148, SAMPLES>::new_for_opus(
        lab_decoder::Decoder::new().unwrap(),
        producer,
        0,
        OpusFrameConfig::new(48_000, 2, 10).unwrap(),
    )
    .unwrap();
    worker.admit(0, 0, &packet[..length]).unwrap();
    worker.admit(2, 960, &packet[..length]).unwrap();
    assert!(matches!(
        worker.decode_next(),
        Ok(DecodeStep::Decoded { samples: 960, .. })
    ));
    assert!(matches!(
        worker.decode_next(),
        Ok(DecodeStep::Concealed { samples: 960, .. })
    ));
    let mut block = [0.0; SAMPLES];
    assert!(matches!(
        consumer.pop_into(&mut block),
        sonimesh_audio::RealtimePcmPopOutcome::Filled { samples: 960, .. }
    ));
    assert!(matches!(
        consumer.pop_into(&mut block),
        sonimesh_audio::RealtimePcmPopOutcome::Filled { samples: 960, .. }
    ));
}

#[derive(Default)]
struct MemoryOutput {
    source: Option<Box<dyn RenderSource>>,
}

impl AudioOutput for MemoryOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn start(
        &mut self,
        _device: Option<&OutputDeviceId>,
        _config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        self.source = Some(source);
        Ok(())
    }

    fn stop(&mut self) {
        self.source = None;
    }

    fn health(&self) -> OutputHealth {
        OutputHealth::Healthy
    }
}

fn read_reply(stream: &mut TcpStream) -> Envelope {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    assert!(length <= MAX_CONTROL_FRAME_BYTES);
    let mut frame = Vec::from(prefix);
    frame.resize(4 + length, 0);
    stream.read_exact(&mut frame[4..]).unwrap();
    decode_control_frame(&frame).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)] // Exercises lifecycle, protected UDP, native decode and output together.
fn scheduled_protected_opus_reaches_memory_output_and_conceals_loss() {
    // The proof result is supplied by the test; transport pairing is tested separately.
    let mut state = SessionState::IDLE;
    let mut pairing = PairingInvitationController::<1>::new();
    pairing
        .begin(&mut state, PairingId::new([1; 16]), 100, 50, 1)
        .unwrap();
    assert_eq!(
        pairing.verify_proof(&mut state, 101, PairingSource::new([2; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );
    let secret = SessionSecret::new([7; 32]);
    let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let (producer, _) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
    let worker = StreamDecodeWorker::<_, 8, 1_148, SAMPLES>::new_for_opus(
        lab_decoder::Decoder::new().unwrap(),
        producer,
        0,
        OpusFrameConfig::new(48_000, 2, 20).unwrap(),
    )
    .unwrap();
    let pipeline = UdpReceiverPipeline::bind_with_worker(
        "127.0.0.1:0".parse().unwrap(),
        &secret,
        context,
        worker,
        Arc::clone(&counters),
    )
    .unwrap();
    let address = pipeline.local_addr().unwrap();
    let bridge = ControlToMixerBridge::new(
        state,
        &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
        MemoryOutput::default(),
        OutputSelection {
            device: None,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: Some(480),
                maximum_callback_frames: 1_024,
            },
        },
        8,
        8,
    )
    .unwrap();
    let owner = DesktopControlOwner::new(bridge).unwrap();
    let mut scheduler: DesktopReceiverScheduler<_, _, 1, 256, 1, SAMPLES, TcpStream> =
        DesktopReceiverScheduler::new(DesktopReceiverOwner::new(owner, pipeline, Some(counters)));
    let session = scheduler
        .receiver_mut()
        .owner_mut()
        .install_authenticated_control_session()
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client.set_nodelay(true).unwrap();
    let (server, _) = listener.accept().unwrap();
    server.set_nonblocking(true).unwrap();
    scheduler.attach_control(session, server).unwrap();
    let hello = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![
                Capability::FrameDuration10Ms as i32,
                Capability::FrameDuration20Ms as i32,
            ],
            display_name: "loopback sender".to_owned(),
        })),
    };
    let start = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 2,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 1,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0,
        })),
    };
    client
        .write_all(&encode_control_frame(&hello).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match scheduler.poll_control_once().unwrap() {
            Some(ControlSessionPoll::Processed { .. }) => break,
            Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            result => panic!("hello did not complete: {result:?}"),
        }
    }
    let reply = read_reply(&mut client);
    assert_eq!(reply.request_id, hello.request_id);
    assert!(matches!(reply.payload, Some(envelope::Payload::Hello(_))));
    for (sample_rate, channels, duration) in [(44_100, 2, 20), (48_000, 1, 20), (48_000, 2, 10)] {
        let mut mismatch = start.clone();
        let Some(envelope::Payload::StartStream(request)) = mismatch.payload.as_mut() else {
            unreachable!()
        };
        request.sample_rate_hz = sample_rate;
        request.channels = channels;
        request.frame_duration_ms = duration;
        assert!(matches!(
            scheduler
                .receiver_mut()
                .accept_and_execute(session, &mismatch),
            Err(sonimesh_app::DesktopControlOwnerError::MediaFormatMismatch)
        ));
    }
    client
        .write_all(&encode_control_frame(&start).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match scheduler.poll_control_once().unwrap() {
            Some(ControlSessionPoll::Processed { .. }) => break,
            Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            result => panic!("start did not complete: {result:?}"),
        }
    }
    let reply = read_reply(&mut client);
    assert_eq!(reply.request_id, start.request_id);
    assert!(
        matches!(reply.payload, Some(envelope::Payload::StreamStatus(status))
        if status.state == StreamState::Streaming as i32)
    );

    let mut sender =
        MediaSessionSender::bind("127.0.0.1:0".parse().unwrap(), &secret, context, address)
            .unwrap();
    let mut encoder =
        opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
    let mut pcm = [0.0_f32; SAMPLES];
    for (index, stereo) in pcm.chunks_exact_mut(2).enumerate() {
        let phase = u16::try_from(index % 48).unwrap();
        let value = (f32::from(phase) * std::f32::consts::TAU / 48.0).sin() * 0.2;
        stereo.fill(value);
    }
    let mut packet = [0_u8; 1_148];
    for sequence in 0..3 {
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        sender
            .send_frame(0, sequence * 960, &packet[..len])
            .unwrap();
    }
    for (now, expected) in [(100, 0), (120, 1), (140, 2)] {
        let tick = scheduler.tick_scheduled_at(now);
        assert_eq!(tick.state, ScheduledReceiverState::Active);
        assert!(tick.control.is_ok() && tick.packets.is_ok() && tick.pump.is_ok());
        assert!(
            matches!(tick.playout, Ok(Some(DecodeStep::Decoded { sequence, .. })) if sequence == expected)
        );
    }
    let tick = scheduler.tick_scheduled_at(160);
    assert!(matches!(
        tick.playout,
        Ok(Some(DecodeStep::Concealed { sequence: 3, .. }))
    ));
    assert!(tick.pump.is_ok());

    let output = scheduler
        .receiver_mut()
        .owner_mut()
        .bridge_mut()
        .mixer_mut()
        .output_mut();
    let mut samples = [0.0; 256];
    let mut peak = 0.0_f32;
    for _ in 0..16 {
        output.source.as_mut().unwrap().render(&mut samples);
        assert!(samples.iter().all(|sample| sample.is_finite()));
        peak = samples
            .iter()
            .fold(peak, |peak, sample| peak.max(sample.abs()));
    }
    assert!(peak > 0.001, "decoded audio must reach the output");

    for (now, expected) in [(180, 4), (200, 5)] {
        assert!(matches!(
            scheduler.tick_scheduled_at(now).playout,
            Ok(Some(DecodeStep::Concealed { sequence, .. })) if sequence == expected
        ));
    }
    for now in (220..=400).step_by(20) {
        let tick = scheduler.tick_scheduled_at(now);
        assert!(tick.pump.is_ok());
        assert!(matches!(tick.playout, Ok(Some(DecodeStep::NotReady))));
    }
    let len = encoder.encode_float(&pcm, &mut packet).unwrap();
    sender.send_frame(0, 3 * 960, &packet[..len]).unwrap();
    let resumed = scheduler.tick_scheduled_at(420);
    assert!(resumed.packets.is_ok() && resumed.pump.is_ok());
    assert!(matches!(
        resumed.playout,
        Ok(Some(DecodeStep::Decoded { sequence: 3, .. }))
    ));

    let sink = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sender.set_peer(sink.local_addr().unwrap());
    for sequence in 4..8 {
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        sender
            .send_frame(0, sequence * 960, &packet[..len])
            .unwrap();
    }
    sender.set_peer(address);
    let len = encoder.encode_float(&pcm, &mut packet).unwrap();
    sender.send_frame(0, 8 * 960, &packet[..len]).unwrap();
    for (now, expected) in [(440, 4), (460, 5), (480, 6), (500, 7)] {
        assert!(matches!(
            scheduler.tick_scheduled_at(now).playout,
            Ok(Some(DecodeStep::Concealed { sequence, .. })) if sequence == expected
        ));
    }
    assert!(matches!(
        scheduler.tick_scheduled_at(520).playout,
        Ok(Some(DecodeStep::Decoded { sequence: 8, .. }))
    ));

    let stop = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 3,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: 7,
            stream_id: 1,
            media_key_epoch: 0,
        })),
    };
    client
        .write_all(&encode_control_frame(&stop).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let tick = scheduler.tick_scheduled_at(521);
        assert!(tick.packets.is_ok() && tick.pump.is_ok());
        match tick.control.unwrap() {
            Some(ControlSessionPoll::Processed { .. }) => {
                assert_eq!(tick.state, ScheduledReceiverState::Idle);
                break;
            }
            Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            result => panic!("stop did not complete: {result:?}"),
        }
    }
    let reply = read_reply(&mut client);
    assert_eq!(reply.request_id, stop.request_id);
    assert!(
        matches!(reply.payload, Some(envelope::Payload::StreamStatus(status))
        if status.state == StreamState::Stopped as i32)
    );
}
