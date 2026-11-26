//! Software-only integration of the native lab decoder and desktop scheduler.

#[path = "../src/bin/lab_decoder/mod.rs"]
mod lab_decoder;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonimesh_app::{
    ControlSessionPoll, ControlToMixerBridge, DesktopControlOwner, DesktopReceiverOwner,
    DesktopReceiverScheduler, MediaSessionAllocator, OutputSelection, PairingAttemptOutcome,
    PairingId, PairingInvitationController, PairingProofVerification, PairingSource,
    ScheduledReceiverState, SessionState, UdpReceiverPipeline,
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

#[cfg(all(windows, feature = "playback"))]
struct MeteredOutput {
    inner: sonimesh_platform::CpalOutput,
    heard_samples: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(all(windows, feature = "playback"))]
struct MeteredSource {
    inner: Box<dyn RenderSource>,
    heard_samples: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(all(windows, feature = "playback"))]
impl RenderSource for MeteredSource {
    fn render(&mut self, output: &mut [f32]) {
        self.inner.render(output);
        if output.iter().any(|sample| sample.abs() > 0.001) {
            self.heard_samples
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

#[cfg(all(windows, feature = "playback"))]
impl AudioOutput for MeteredOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        self.inner.devices()
    }

    fn start(
        &mut self,
        device: Option<&OutputDeviceId>,
        config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        self.inner.start(
            device,
            config,
            Box::new(MeteredSource {
                inner: source,
                heard_samples: Arc::clone(&self.heard_samples),
            }),
        )
    }

    fn stop(&mut self) {
        self.inner.stop();
    }

    fn health(&self) -> OutputHealth {
        self.inner.health()
    }
}

#[cfg(all(windows, feature = "playback"))]
#[test]
#[ignore = "opens the default Windows audio output and plays a short test signal"]
fn paired_tls_scheduler_reaches_windows_output_callback() {
    let output = sonimesh_platform::CpalOutput::new();
    let probe = output
        .default_device_probe()
        .expect("default output is available");
    let config = probe
        .output_config(None, 8_192)
        .expect("output format is supported");
    println!(
        "CPAL output: {} Hz, {} channels, {:?}",
        config.sample_rate_hz, config.channels, config.sample_format
    );
    let selection = OutputSelection {
        device: Some(probe.device.id),
        config,
    };
    let meter = Arc::new(std::sync::atomic::AtomicBool::new(false));
    paired_tls_scheduler_output(
        MeteredOutput {
            inner: output,
            heard_samples: meter,
        },
        selection,
        |output| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !output
                .heard_samples
                .load(std::sync::atomic::Ordering::Relaxed)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            let status = output.inner.status();
            assert!(status.callback_count > 0, "output callback did not run");
            assert!(
                output
                    .heard_samples
                    .load(std::sync::atomic::Ordering::Relaxed),
                "decoded signal did not reach output callback"
            );
            assert_eq!(output.inner.health(), OutputHealth::Healthy);
            println!(
                "CPAL callbacks: {}, largest callback frames: {}",
                status.callback_count, status.maximum_callback_frames
            );
        },
        |output| assert!(!output.inner.is_running()),
    );
}

fn read_reply(stream: &mut impl Read) -> Envelope {
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
fn paired_tls_scheduler_decodes_protected_opus_to_output() {
    paired_tls_scheduler_output(
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
        |output| {
            let mut rendered = [0.0; 256];
            let mut peak = 0.0_f32;
            for _ in 0..16 {
                output.source.as_mut().unwrap().render(&mut rendered);
                peak = rendered
                    .iter()
                    .fold(peak, |peak, sample| peak.max(sample.abs()));
            }
            assert!(peak > 0.001, "paired audio must reach output");
        },
        |output| assert!(output.source.is_none()),
    );
}

#[allow(clippy::too_many_lines)] // Exercises pairing, control, media and output as one path.
fn paired_tls_scheduler_output<O: AudioOutput>(
    output: O,
    selection: OutputSelection,
    verify_output: impl FnOnce(&mut O),
    verify_stopped: impl FnOnce(&mut O),
) {
    use rcgen::generate_simple_self_signed;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use sonimesh_app::{
        InvitationInputs, PairingQrInvitation, accept_pairing_stream, begin_from_rendered_payload,
    };
    use sonimesh_network::{
        PinnedServerChannel, client_config, connect_pinned_client, server_config,
    };
    use sonimesh_protocol::{
        MAX_QR_PAYLOAD_LEN, PROOF_FRAME_LEN, PairingBinding, PairingProofFrame, PairingRole,
    };
    use zeroize::Zeroizing;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let id = PairingId::new([3; 16]);
    let secret = [9; 32];
    let certificate = generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let pin = certificate.key_pair.public_key_der();
    let private = Zeroizing::new(certificate.key_pair.serialize_der());
    let tls = Arc::new(
        server_config(
            certificate.cert.der().clone(),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec())),
        )
        .unwrap(),
    );
    let mut pairing = PairingInvitationController::<4>::new();
    let mut state = SessionState::IDLE;
    let mut payload = vec![0; MAX_QR_PAYLOAD_LEN];
    let length = begin_from_rendered_payload(
        &mut pairing,
        &mut state,
        100,
        60_000,
        2,
        PairingQrInvitation {
            id,
            pairing_secret: secret,
            control_host: "localhost",
            control_port: address.port(),
            desktop_tls_spki: &pin,
            expires_at_ms: 60_100,
        },
        &mut payload,
    )
    .unwrap();
    payload.truncate(length);

    let client_thread = std::thread::spawn(move || {
        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (mut channel, exporter) = connect_pinned_client(
            client_config(pin).unwrap(),
            "localhost",
            &id.as_bytes(),
            stream,
        )
        .unwrap();
        let binding = PairingBinding::new(1, id.as_bytes(), secret, exporter.as_ref()).unwrap();
        let proof = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: id.as_bytes(),
            proof: binding.local_proof(PairingRole::AndroidClient).unwrap(),
        };
        let mut bytes = [0; PROOF_FRAME_LEN];
        proof.encode_into(&mut bytes).unwrap();
        channel.write_all(&bytes).unwrap();
        channel.flush().unwrap();
        channel.read_exact(&mut bytes).unwrap();
        let reply = sonimesh_protocol::parse_pairing_proof_frame(&bytes).unwrap();
        assert!(
            binding
                .verify_peer_proof(PairingRole::AndroidClient, &reply.proof)
                .unwrap()
        );
        (channel, binding.media_root().unwrap())
    });
    let (stream, _) = listener.accept().unwrap();
    let (trusted, root) = accept_pairing_stream(
        stream,
        tls,
        &mut pairing,
        &mut state,
        100,
        PairingSource::new([4; 16]),
        &InvitationInputs {
            invitation: id,
            pairing_secret: secret,
            scanned_payload: &payload,
        },
    )
    .unwrap();
    let (mut client, client_root) = client_thread.join().unwrap();
    let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
    let mut allocator = MediaSessionAllocator::new(root);
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
        &SessionSecret::new([0x55; 32]),
        MediaKeyContext::new(8, 1, 0, MediaDirection::AndroidToDesktop).unwrap(),
        worker,
        Arc::clone(&counters),
    )
    .unwrap();
    let media_address = pipeline.local_addr().unwrap();
    let bridge = ControlToMixerBridge::new(
        state,
        &[Capability::FrameDuration10Ms, Capability::FrameDuration20Ms],
        output,
        selection,
        8,
        8,
    )
    .unwrap();
    let owner = DesktopControlOwner::new(bridge).unwrap();
    let mut scheduler: DesktopReceiverScheduler<_, _, 1, 256, 1, SAMPLES, PinnedServerChannel> =
        DesktopReceiverScheduler::new(DesktopReceiverOwner::new(owner, pipeline, Some(counters)));
    scheduler.set_scheduled_startup_frames(2).unwrap();
    let token = scheduler
        .receiver_mut()
        .owner_mut()
        .install_authenticated_control_session()
        .unwrap();
    scheduler.attach_trusted_control(token, trusted).unwrap();
    let mut prepare =
        |token,
         request: sonimesh_app::StreamStartRequest,
         coordinator: &sonimesh_app::ControlStreamCoordinator,
         pipeline: &mut UdpReceiverPipeline<lab_decoder::Decoder, 8, 1_148, SAMPLES>| {
            let grant = allocator
                .supply_for_authenticated_control_session(
                    coordinator,
                    token,
                    request.identity,
                    MediaDirection::AndroidToDesktop,
                )
                .map_err(|_| sonimesh_app::DesktopControlOwnerError::MediaContextMismatch)?;
            let frame = OpusFrameConfig::new(
                request.sample_rate_hz,
                u16::try_from(request.channels).unwrap(),
                u16::try_from(request.frame_duration_ms).unwrap(),
            )
            .unwrap();
            pipeline
                .install_fresh_opus_session(grant.root(), grant.context(), 0, frame)
                .map_err(|_| sonimesh_app::DesktopControlOwnerError::MediaContextMismatch)
        };
    for envelope in [
        Envelope {
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
        },
        Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 2,
                frame_duration_ms: 10,
                media_key_epoch: 0,
            })),
        },
    ] {
        client
            .write_all(&encode_control_frame(&envelope).unwrap())
            .unwrap();
        client.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let tick = scheduler.tick_scheduled_at_with_prepare(0, &mut prepare);
            assert!(tick.packets.is_ok() && tick.pump.is_ok());
            match tick.control.unwrap() {
                Some(ControlSessionPoll::Processed { .. }) => break,
                Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Some(ControlSessionPoll::Rejected { .. }) => {
                    panic!("control rejected: {:?}", read_reply(&mut client))
                }
                result => panic!("control did not complete: {result:?}"),
            }
        }
        let reply = read_reply(&mut client);
        assert_eq!(reply.request_id, envelope.request_id);
        if envelope.request_id == 2 {
            assert!(
                matches!(reply.payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Streaming as i32)
            );
        }
    }
    assert_eq!(allocator.issued_count(), 1);
    assert_eq!(scheduler.receiver_mut().pipeline_mut().context(), context);
    let mut sender = MediaSessionSender::bind(
        "127.0.0.1:0".parse().unwrap(),
        &client_root,
        context,
        media_address,
    )
    .unwrap();
    let mut encoder =
        opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
    let mut packet = [0; 1_148];
    let samples = std::array::from_fn::<_, 960, _>(|i| {
        (f32::from(u16::try_from(i / 2 % 48).unwrap()) * std::f32::consts::TAU / 48.0).sin() * 0.2
    });
    for sequence in 0..3 {
        let length = encoder.encode_float(&samples, &mut packet).unwrap();
        sender
            .send_frame(0, sequence * 480, &packet[..length])
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut received = 0;
    while received < 3 {
        let tick = scheduler.tick_scheduled_at(0);
        received += tick.packets.unwrap();
        assert!(matches!(tick.playout, Ok(None)) && tick.pump.is_ok());
        assert!(Instant::now() < deadline, "UDP packets did not arrive");
        std::thread::yield_now();
    }
    for (now, expected) in [(20, 0), (30, 1), (40, 2)] {
        let tick = scheduler.tick_scheduled_at(now);
        assert_eq!(tick.state, ScheduledReceiverState::Active);
        assert!(tick.control.is_ok() && tick.packets.is_ok() && tick.pump.is_ok());
        assert!(
            matches!(tick.playout, Ok(Some(DecodeStep::Decoded { sequence, .. })) if sequence == expected)
        );
    }
    let output = scheduler
        .receiver_mut()
        .owner_mut()
        .bridge_mut()
        .mixer_mut()
        .output_mut();
    verify_output(output);
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
    client.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match scheduler.tick_scheduled_at(40).control.unwrap() {
            Some(ControlSessionPoll::Processed { .. }) => break,
            Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                std::thread::yield_now();
            }
            result => panic!("stop did not complete: {result:?}"),
        }
    }
    let reply = read_reply(&mut client);
    assert_eq!(reply.request_id, 3);
    assert!(
        matches!(reply.payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Stopped as i32)
    );
    verify_stopped(
        scheduler
            .receiver_mut()
            .owner_mut()
            .bridge_mut()
            .mixer_mut()
            .output_mut(),
    );
}

#[test]
fn scheduled_protected_opus_reaches_memory_output_and_conceals_loss() {
    scheduled_protected_opus(false, false);
}

#[test]
fn buffered_scheduled_opus_waits_for_startup_and_recovery() {
    scheduled_protected_opus(true, false);
}

#[test]
fn scheduled_start_prepares_a_fresh_session_and_retries_after_failure() {
    scheduled_protected_opus(false, true);
}

#[allow(clippy::too_many_lines)] // Exercises lifecycle, protected UDP, native decode and output together.
fn scheduled_protected_opus(buffered: bool, dynamic: bool) {
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
        if dynamic {
            MediaKeyContext::new(8, 1, 0, MediaDirection::AndroidToDesktop).unwrap()
        } else {
            context
        },
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
    if buffered {
        scheduler.set_scheduled_startup_frames(2).unwrap();
    }
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
    for (sample_rate, channels, duration) in [(44_100, 2, 20), (48_000, 1, 20)] {
        let mut mismatch = start.clone();
        let Some(envelope::Payload::StartStream(request)) = mismatch.payload.as_mut() else {
            unreachable!()
        };
        request.sample_rate_hz = sample_rate;
        request.channels = channels;
        request.frame_duration_ms = duration;
        assert!(matches!(
            scheduler.receiver_mut().accept_and_execute_with_prepare(
                session,
                &mismatch,
                &mut |_, _, _, _| { panic!("unsupported format must not prepare media") }
            ),
            Err(sonimesh_app::DesktopControlOwnerError::MediaFormatMismatch)
        ));
    }
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([7; 32]));
    let mut preparation_calls = 0;
    let mut prepare =
        |token,
         request: sonimesh_app::StreamStartRequest,
         coordinator: &sonimesh_app::ControlStreamCoordinator,
         pipeline: &mut UdpReceiverPipeline<lab_decoder::Decoder, 8, 1_148, SAMPLES>| {
            preparation_calls += 1;
            if preparation_calls == 1 {
                return Err(sonimesh_app::DesktopControlOwnerError::MediaContextMismatch);
            }
            let grant = allocator
                .supply_for_authenticated_control_session(
                    coordinator,
                    token,
                    request.identity,
                    MediaDirection::AndroidToDesktop,
                )
                .map_err(|_| sonimesh_app::DesktopControlOwnerError::MediaContextMismatch)?;
            pipeline
                .install_fresh_session(grant.root(), grant.context(), 0)
                .map_err(|_| sonimesh_app::DesktopControlOwnerError::MediaContextMismatch)
        };
    if dynamic {
        client
            .write_all(&encode_control_frame(&start).unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let tick = scheduler.tick_scheduled_at_with_prepare(0, &mut prepare);
            match tick.control.unwrap() {
                Some(ControlSessionPoll::Rejected { .. }) => break,
                Some(ControlSessionPoll::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                result => panic!("failed preparation was not rejected: {result:?}"),
            }
        }
        let reply = read_reply(&mut client);
        assert!(matches!(reply.payload, Some(envelope::Payload::Error(error)) if error.retryable));
        assert_eq!(
            scheduler.receiver_mut().pipeline_mut().context().session_id,
            8
        );
    }
    client
        .write_all(&encode_control_frame(&start).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = if dynamic {
            scheduler
                .tick_scheduled_at_with_prepare(80, &mut prepare)
                .control
                .unwrap()
        } else {
            scheduler.poll_control_once().unwrap()
        };
        match result {
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
    if dynamic {
        assert_eq!(preparation_calls, 2);
        assert_eq!(allocator.issued_count(), 1);
        assert_eq!(scheduler.receiver_mut().pipeline_mut().context(), context);
    }

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
    if buffered {
        assert!(matches!(scheduler.tick_scheduled_at(80).playout, Ok(None)));
    }
    for sequence in 0..3 {
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        sender
            .send_frame(0, sequence * 960, &packet[..len])
            .unwrap();
        if buffered && sequence < 2 {
            let tick = scheduler.tick_scheduled_at(100 + sequence * 20);
            assert_eq!(tick.packets.unwrap(), 1);
            assert!(matches!(tick.playout, Ok(None)));
            if sequence == 1 {
                let mut samples = [1.0; 256];
                scheduler
                    .receiver_mut()
                    .owner_mut()
                    .bridge_mut()
                    .mixer_mut()
                    .output_mut()
                    .source
                    .as_mut()
                    .unwrap()
                    .render(&mut samples);
                assert!(samples.iter().all(|sample| *sample == 0.0));
            }
        }
    }
    let startup_delay = if buffered { 40 } else { 0 };
    for (now, expected) in [(100, 0), (120, 1), (140, 2)] {
        let tick = scheduler.tick_scheduled_at(now + startup_delay);
        assert_eq!(tick.state, ScheduledReceiverState::Active);
        assert!(tick.control.is_ok() && tick.packets.is_ok() && tick.pump.is_ok());
        assert!(
            matches!(tick.playout, Ok(Some(DecodeStep::Decoded { sequence, .. })) if sequence == expected)
        );
    }
    let tick = scheduler.tick_scheduled_at(160 + startup_delay);
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
            scheduler.tick_scheduled_at(now + startup_delay).playout,
            Ok(Some(DecodeStep::Concealed { sequence, .. })) if sequence == expected
        ));
    }
    for now in (220..=400).step_by(20) {
        let tick = scheduler.tick_scheduled_at(now + startup_delay);
        assert!(tick.pump.is_ok());
        if buffered {
            assert!(matches!(tick.playout, Ok(None)));
        } else {
            assert!(matches!(tick.playout, Ok(Some(DecodeStep::NotReady))));
        }
    }
    let len = encoder.encode_float(&pcm, &mut packet).unwrap();
    sender.send_frame(0, 3 * 960, &packet[..len]).unwrap();
    let resumed_at = 420 + startup_delay;
    if buffered {
        let waiting = scheduler.tick_scheduled_at(resumed_at);
        assert_eq!(waiting.packets.unwrap(), 1);
        assert!(matches!(waiting.playout, Ok(None)));
        assert!(matches!(
            scheduler.tick_scheduled_at(resumed_at + 39).playout,
            Ok(None)
        ));
    }
    let resumed = scheduler.tick_scheduled_at(resumed_at + startup_delay);
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
            scheduler.tick_scheduled_at(now + 2 * startup_delay).playout,
            Ok(Some(DecodeStep::Concealed { sequence, .. })) if sequence == expected
        ));
    }
    assert!(matches!(
        scheduler.tick_scheduled_at(520 + 2 * startup_delay).playout,
        Ok(Some(DecodeStep::Decoded { sequence: 8, .. }))
    ));

    let mut stop_at = 521 + 2 * startup_delay;
    if buffered {
        let identity = sonimesh_app::MediaStreamIdentity {
            session_id: 7,
            stream_id: 1,
            key_epoch: 0,
        };
        assert!(
            scheduler
                .receiver()
                .owner()
                .bridge()
                .mixer()
                .queued_output_blocks(identity)
                .unwrap()
                > 0
        );
        sender.send_frame(0, 9 * 960, &[0xff]).unwrap();
        let failed_at = 540 + 2 * startup_delay;
        let failure = scheduler.tick_scheduled_at(failed_at);
        assert!(failure.pump.is_ok());
        assert!(matches!(
            failure.playout,
            Err(sonimesh_app::ScheduledPlayoutError::Decode(_))
        ));
        assert_eq!(
            scheduler
                .receiver()
                .owner()
                .bridge()
                .mixer()
                .queued_output_blocks(identity)
                .unwrap(),
            0
        );
        let len = encoder.encode_float(&pcm, &mut packet).unwrap();
        sender.send_frame(0, 10 * 960, &packet[..len]).unwrap();
        let first_fresh = scheduler.tick_scheduled_at(failed_at + 1);
        assert_eq!(first_fresh.packets.unwrap(), 1);
        assert!(matches!(first_fresh.playout, Ok(None)));
        assert!(matches!(
            scheduler.tick_scheduled_at(failed_at + 40).playout,
            Ok(None)
        ));
        assert!(matches!(
            scheduler.tick_scheduled_at(failed_at + 41).playout,
            Ok(Some(DecodeStep::Decoded { sequence: 10, .. }))
        ));
        stop_at = failed_at + 42;
    }

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
        let tick = scheduler.tick_scheduled_at(stop_at);
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
