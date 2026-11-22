//! Paired Android-to-desktop UDP audio software-render lab.
//! The fixed ID and secret are public test data, never product credentials.
mod lab_decoder;

use std::fmt::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sonimesh_app::{
    ControlReply, ControlStreamAction, ControlStreamCoordinator, InvitationInputs,
    MediaSessionAllocator, PairingId, PairingInvitationController, PairingListener,
    PairingQrInvitation, PairingSource, SessionState, UdpPollOutcome, UdpReceiverPipeline,
    accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_audio::{DecodeStep, OpusFrameConfig, StreamDecodeWorker, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{
    UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite, server_config,
};
use sonimesh_protocol::{
    Capability, Envelope, Hello, MAX_QR_PAYLOAD_LEN, MediaDirection, SessionSecret, StreamState,
    envelope,
};
use zeroize::{Zeroize, Zeroizing};

const LAB_ID: [u8; 16] = [0x71; 16];
const LAB_SECRET: [u8; 32] = [0x42; 32];
type LabResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("pairing-lab: {error}");
        std::process::exit(1);
    }
}

fn run() -> LabResult<()> {
    let listener = PairingListener::bind("127.0.0.1:47610")?;
    let address = listener.local_addr()?;
    let key = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let pin = key.key_pair.public_key_der();
    let mut private = Zeroizing::new(key.key_pair.serialize_der());
    let tls = Arc::new(server_config(
        key.cert.der().clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec())),
    )?);
    private.zeroize();
    let id = PairingId::new(LAB_ID);
    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload = vec![0; MAX_QR_PAYLOAD_LEN];
    let length = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        100,
        120_000,
        2,
        PairingQrInvitation {
            id,
            pairing_secret: LAB_SECRET,
            control_host: "127.0.0.1",
            control_port: address.port(),
            desktop_tls_spki: &pin,
            expires_at_ms: 120_100,
        },
        &mut payload,
    )?;
    payload.truncate(length);
    let mut pin_hex = String::with_capacity(pin.len() * 2);
    for byte in &pin {
        write!(pin_hex, "{byte:02x}")?;
    }
    println!("SPKI_HEX {pin_hex}");
    println!("LISTENING {address}");
    println!("READY");
    let started = Instant::now();
    let stream = loop {
        if let Some(stream) = listener.try_accept()? {
            break stream;
        }
        if started.elapsed() >= Duration::from_secs(90) {
            return Err("client connection timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let elapsed = u64::try_from(started.elapsed().as_millis())?;
    let (channel, root) = accept_pairing_stream(
        stream,
        tls,
        &mut pairing,
        &mut session,
        100 + elapsed,
        PairingSource::new([0x72; 16]),
        &InvitationInputs {
            invitation: id,
            pairing_secret: LAB_SECRET,
            scanned_payload: &payload,
        },
    )?;
    println!("RESULT pairing verified");
    negotiate_hello(channel, session, root)
}

fn negotiate_hello(
    channel: sonimesh_app::TrustedControlChannel,
    session: SessionState,
    root: SessionSecret,
) -> LabResult<()> {
    let (channel, _exporter) = channel.into_parts();
    let socket = channel.sock.try_clone()?;
    channel
        .sock
        .set_read_timeout(Some(Duration::from_secs(30)))?;
    channel
        .sock
        .set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut framed = UntrustedControlStream::from_io(channel);
    let mut coordinator = ControlStreamCoordinator::new(session, &[Capability::FrameDuration20Ms])?;
    let token = coordinator.install_authenticated_control_session()?;
    let request = loop {
        match framed.poll()? {
            UntrustedControlPoll::Frame(envelope) => break envelope,
            UntrustedControlPoll::Empty => continue,
            UntrustedControlPoll::Closed => {
                return Err("control channel closed before hello".into());
            }
        }
    };
    if !matches!(
        coordinator.accept(token, &request)?,
        ControlStreamAction::Negotiated(_)
    ) {
        return Err("expected a valid hello".into());
    }
    let reply = Envelope {
        protocol_version: 1,
        request_id: request.request_id,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![1],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "desktop lab".to_owned(),
        })),
    };
    let mut state = framed.send(&reply)?;
    while state == UntrustedControlWrite::Pending {
        state = framed.flush_write()?;
    }
    if state != UntrustedControlWrite::Complete {
        return Err("control channel closed during hello reply".into());
    }
    println!("RESULT hello negotiated");
    socket.set_read_timeout(None)?;
    socket.set_write_timeout(None)?;
    socket.set_nonblocking(true)?;
    receive_audio(&mut framed, &mut coordinator, token, root)
}

const SAMPLES: usize = 1_920;
type Receiver = UdpReceiverPipeline<lab_decoder::Decoder, 8, 1_148, SAMPLES>;

#[allow(clippy::too_many_lines)] // This one-shot lab owns the control and media lifetimes together.
fn receive_audio<S: std::io::Read + std::io::Write>(
    framed: &mut UntrustedControlStream<S>,
    coordinator: &mut ControlStreamCoordinator,
    control_token: sonimesh_app::ControlSessionToken,
    root: SessionSecret,
) -> LabResult<()> {
    let mut allocator = MediaSessionAllocator::new(root);
    let mut receiver: Option<Receiver> = None;
    let mut consumer = None;
    let mut counters = None;
    let mut next_playout = None;
    let mut decoded = 0_u64;
    let mut peak = 0_f32;
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        match framed.poll()? {
            UntrustedControlPoll::Frame(frame) => {
                let action = match coordinator.accept(control_token, &frame) {
                    Ok(action) => action,
                    Err(error) => {
                        send_reply(
                            framed,
                            &ControlReply::for_rejection(frame.request_id, &error)?,
                        )?;
                        continue;
                    }
                };
                let reply = match action {
                    ControlStreamAction::Start { token, request } => {
                        if receiver.is_some()
                            || (
                                request.sample_rate_hz,
                                request.channels,
                                request.frame_duration_ms,
                            ) != (48_000, 2, 20)
                        {
                            coordinator.abort_pending(token)?;
                            ControlReply::error(
                                sonimesh_protocol::ErrorCode::UnsupportedCapability,
                                false,
                                "lab requires one 48 kHz stereo 20 ms stream",
                            )?
                        } else {
                            let grant = allocator.supply_for_authenticated_control_session(
                                coordinator,
                                control_token,
                                request.identity,
                                MediaDirection::AndroidToDesktop,
                            )?;
                            let metrics = Arc::new(StreamCounters::default());
                            let (producer, next_consumer) = realtime_pcm_queue::<SAMPLES>(8)?;
                            let worker = StreamDecodeWorker::new_for_opus(
                                lab_decoder::Decoder::new()?,
                                producer,
                                0,
                                OpusFrameConfig::new(48_000, 2, 20)?,
                            )?;
                            let pipeline = Receiver::bind_with_worker(
                                "127.0.0.1:39093".parse()?,
                                grant.root(),
                                grant.context(),
                                worker,
                                Arc::clone(&metrics),
                            )?;
                            receiver = Some(pipeline);
                            consumer = Some(next_consumer);
                            counters = Some(metrics);
                            coordinator.complete_start(token, request)?;
                            println!("RESULT receiver started");
                            ControlReply::stream_status(
                                request.identity,
                                StreamState::Streaming,
                                "receiver ready",
                            )?
                        }
                    }
                    ControlStreamAction::Stop { token, identity } => {
                        drop(receiver.take());
                        drop(consumer.take());
                        drop(counters.take());
                        coordinator.complete_stop(token, identity)?;
                        let reply = ControlReply::stream_status(
                            identity,
                            StreamState::Stopped,
                            "receiver stopped",
                        )?
                        .into_envelope(frame.request_id)?;
                        send_reply(framed, &reply)?;
                        println!("RESULT decoded {decoded} peak {peak:.3}");
                        if decoded == 0 || peak < 0.001 {
                            return Err("no non-silent audio decoded".into());
                        }
                        return Ok(());
                    }
                    _ => ControlReply::error(
                        sonimesh_protocol::ErrorCode::InvalidRequest,
                        false,
                        "unsupported lab command",
                    )?,
                };
                send_reply(framed, &reply.into_envelope(frame.request_id)?)?;
            }
            UntrustedControlPoll::Closed => return Err("control channel closed".into()),
            UntrustedControlPoll::Empty => {}
        }
        if let (Some(pipeline), Some(pcm), Some(metrics)) =
            (&mut receiver, &mut consumer, &counters)
        {
            for _ in 0..32 {
                match pipeline.poll()? {
                    UdpPollOutcome::Empty => break,
                    UdpPollOutcome::Processed { .. } => {
                        if next_playout.is_none() {
                            next_playout = Some(Instant::now() + Duration::from_millis(60));
                        }
                    }
                }
            }
            let now = Instant::now();
            if next_playout.is_some_and(|due| now >= due) {
                if metrics.snapshot().jitter_frames > 0
                    && matches!(pipeline.decode_next()?, DecodeStep::Decoded { .. })
                {
                    decoded += 1;
                }
                let mut samples = [0_f32; SAMPLES];
                pcm.pop_into(&mut samples);
                for sample in samples {
                    peak = peak.max(sample.abs());
                }
                next_playout = Some(now + Duration::from_millis(20));
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err("paired audio lab timed out".into())
}

fn send_reply<S: std::io::Read + std::io::Write>(
    framed: &mut UntrustedControlStream<S>,
    reply: &Envelope,
) -> LabResult<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut state = framed.send(reply)?;
    while state == UntrustedControlWrite::Pending && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
        state = framed.flush_write()?;
    }
    if state != UntrustedControlWrite::Complete {
        return Err("control reply failed".into());
    }
    Ok(())
}
