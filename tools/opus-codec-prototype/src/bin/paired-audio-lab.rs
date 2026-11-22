//! Paired Android-to-desktop UDP audio lab.
//! The fixed ID and secret are public test data, never product credentials.
#[cfg(all(windows, feature = "playback"))]
#[allow(dead_code)]
mod common;
mod lab_decoder;

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sonimesh_app::{
    ControlReply, ControlStreamAction, ControlStreamCoordinator, InvitationInputs,
    MediaSessionAllocator, PairingId, PairingInvitationController, PairingListener,
    PairingQrInvitation, PairingSource, ReceiverPipelineError, SessionState, UdpPollOutcome,
    UdpReceiverError, UdpReceiverPipeline, accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, OpusFrameConfig, StreamDecodeWorker,
    realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{
    IngressError, UdpReceiveError, UntrustedControlPoll, UntrustedControlStream,
    UntrustedControlWrite, server_config,
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
    let options = Options::parse(std::env::args().skip(1))?;
    let playback = Playback::prepare(options.play, options.select_output)?;
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
    negotiate_hello(channel, session, root, options.require_tone, playback)
}

#[derive(Default)]
struct Options {
    require_tone: bool,
    play: bool,
    select_output: bool,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> LabResult<Self> {
        let mut result = Self::default();
        for arg in args {
            match arg.as_str() {
                "--require-tone" if !result.require_tone => result.require_tone = true,
                "--play" if !result.play => result.play = true,
                "--select-output" if !result.select_output => result.select_output = true,
                _ => {
                    return Err(
                        "usage: paired-audio-lab [--require-tone] [--play [--select-output]]"
                            .into(),
                    );
                }
            }
        }
        if result.select_output && !result.play {
            return Err("--select-output requires --play".into());
        }
        Ok(result)
    }
}

fn negotiate_hello(
    channel: sonimesh_app::TrustedControlChannel,
    session: SessionState,
    root: SessionSecret,
    require_tone: bool,
    playback: Playback,
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
    receive_audio(
        &mut framed,
        &mut coordinator,
        token,
        root,
        require_tone,
        playback,
    )
}

const SAMPLES: usize = 1_920;
const PLAYOUT_PERIOD: Duration = Duration::from_millis(20);
type Receiver = UdpReceiverPipeline<ObservedDecoder, 8, 1_148, SAMPLES>;

fn poll_media(receiver: &mut Receiver) -> LabResult<Option<UdpPollOutcome>> {
    match receiver.poll() {
        Ok(outcome) => Ok(Some(outcome)),
        Err(
            UdpReceiverError::Socket(UdpReceiveError::DatagramTooLarge)
            | UdpReceiverError::SourceEndpointChanged
            | UdpReceiverError::Pipeline(
                ReceiverPipelineError::Ingress(
                    IngressError::Parse(_)
                    | IngressError::UnknownStream
                    | IngressError::Authentication(_)
                    | IngressError::Replay(_),
                )
                | ReceiverPipelineError::Decode(DecodeWorkerError::Frame(_)),
            ),
        ) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn next_playout_deadline(deadline: Instant, now: Instant) -> Instant {
    if now.duration_since(deadline) >= PLAYOUT_PERIOD {
        now + PLAYOUT_PERIOD
    } else {
        deadline + PLAYOUT_PERIOD
    }
}

struct ObservedDecoder {
    decoder: lab_decoder::Decoder,
    peak: Arc<AtomicU32>,
}

impl FrameDecoder for ObservedDecoder {
    type Error = opus::Error;

    fn decode(&mut self, input: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        let count = self.decoder.decode(input, output)?;
        let peak = output[..count]
            .iter()
            .filter(|sample| sample.is_finite())
            .fold(0_f32, |peak, sample| peak.max(sample.abs()));
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        Ok(count)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        self.decoder.decode_plc(output)
    }

    fn reset(&mut self) {
        self.decoder.reset();
    }
}

#[cfg(all(windows, feature = "playback"))]
struct Playback(
    Option<(
        sonimesh_app::ReceiverOutputController<sonimesh_platform::CpalOutput, SAMPLES>,
        sonimesh_app::OutputSelection,
    )>,
);

#[cfg(not(all(windows, feature = "playback")))]
struct Playback;

#[cfg_attr(
    not(all(windows, feature = "playback")),
    allow(clippy::unused_self, clippy::unnecessary_wraps)
)]
impl Playback {
    fn prepare(play: bool, select: bool) -> LabResult<Self> {
        #[cfg(all(windows, feature = "playback"))]
        {
            if play {
                let (output, selection) =
                    common::prepare_output(select).map_err(|e| e.to_string())?;
                let factory = sonimesh_app::ReceiverPcmEndpointFactory::new(
                    8,
                    sonimesh_app::SourcePcmFormat::new(48_000, 2)?,
                    Arc::new(StreamCounters::default()),
                );
                return Ok(Self(Some((
                    sonimesh_app::ReceiverOutputController::new(output, factory),
                    selection,
                ))));
            }
            Ok(Self(None))
        }
        #[cfg(not(all(windows, feature = "playback")))]
        {
            let _ = select;
            if play {
                return Err("--play requires Windows and the playback feature".into());
            }
            Ok(Self)
        }
    }

    fn start(&mut self, receiver: &mut Receiver) -> LabResult<bool> {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, selection)) = &mut self.0 {
            controller.start(selection.clone(), receiver)?;
            return Ok(true);
        }
        let _ = receiver;
        Ok(false)
    }

    fn monitor(&mut self) -> LabResult<()> {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, _)) = &mut self.0 {
            if let Some(health) = controller.monitor() {
                return Err(format!("output failed: {health:?}").into());
            }
        }
        Ok(())
    }

    fn stop(&mut self) -> LabResult<()> {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, _)) = &mut self.0 {
            let status = controller.output().status();
            controller.stop();
            println!(
                "RESULT output callbacks {} stream errors {} oversized {}",
                status.callback_count, status.stream_errors, status.oversized_callbacks
            );
            if status.callback_count == 0
                || status.stream_errors != 0
                || status.oversized_callbacks != 0
            {
                return Err("output did not render cleanly".into());
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)] // This one-shot lab owns the control and media lifetimes together.
fn receive_audio<S: std::io::Read + std::io::Write>(
    framed: &mut UntrustedControlStream<S>,
    coordinator: &mut ControlStreamCoordinator,
    control_token: sonimesh_app::ControlSessionToken,
    root: SessionSecret,
    require_tone: bool,
    mut playback: Playback,
) -> LabResult<()> {
    let mut allocator = MediaSessionAllocator::new(root);
    let mut receiver: Option<Receiver> = None;
    let mut consumer = None;
    let mut counters = None;
    let mut next_playout = None;
    let mut decoded = 0_u64;
    let mut peak = 0_f32;
    let mut playing = false;
    let decoded_peak = Arc::new(AtomicU32::new(0));
    let deadline = Instant::now() + Duration::from_secs(40);
    while receiver.is_some() || Instant::now() < deadline {
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
                                ObservedDecoder {
                                    decoder: lab_decoder::Decoder::new()?,
                                    peak: Arc::clone(&decoded_peak),
                                },
                                producer,
                                0,
                                OpusFrameConfig::new(48_000, 2, 20)?,
                            )?;
                            let mut pipeline = Receiver::bind_with_worker(
                                "127.0.0.1:39093".parse()?,
                                grant.root(),
                                grant.context(),
                                worker,
                                Arc::clone(&metrics),
                            )?;
                            playing = match playback.start(&mut pipeline) {
                                Ok(playing) => playing,
                                Err(error) => {
                                    eprintln!("output start failed: {error}");
                                    coordinator.abort_pending(token)?;
                                    send_reply(
                                        framed,
                                        &ControlReply::error(
                                            sonimesh_protocol::ErrorCode::RetryLater,
                                            true,
                                            "output unavailable",
                                        )?
                                        .into_envelope(frame.request_id)?,
                                    )?;
                                    continue;
                                }
                            };
                            receiver = Some(pipeline);
                            consumer = if playing { None } else { Some(next_consumer) };
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
                        let output_result = playback.stop();
                        drop(receiver.take());
                        drop(consumer.take());
                        if let Some(metrics) = counters.take() {
                            let snapshot = metrics.snapshot();
                            println!(
                                "RESULT packets {} rejected {} late {} resync_dropped {} plc {} buffered {}",
                                snapshot.authenticated_packets,
                                snapshot.rejected_packets,
                                snapshot.late_packets,
                                snapshot.jitter_resync_dropped_frames,
                                snapshot.plc_frames,
                                snapshot.jitter_frames,
                            );
                        }
                        coordinator.complete_stop(token, identity)?;
                        let reply = ControlReply::stream_status(
                            identity,
                            StreamState::Stopped,
                            "receiver stopped",
                        )?
                        .into_envelope(frame.request_id)?;
                        send_reply(framed, &reply)?;
                        output_result?;
                        if playing {
                            peak = f32::from_bits(decoded_peak.load(Ordering::Relaxed));
                        }
                        println!("RESULT decoded {decoded} peak {peak:.3}");
                        if decoded == 0 || peak < 0.001 {
                            if require_tone {
                                return Err("no non-silent audio decoded".into());
                            }
                            println!(
                                "No capturable audio detected; the source may be silent or capture-limited, or media packets may not have arrived."
                            );
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
        playback.monitor()?;
        if let (Some(pipeline), Some(metrics)) = (&mut receiver, &counters) {
            for _ in 0..32 {
                match poll_media(pipeline)? {
                    None => continue,
                    Some(UdpPollOutcome::Empty) => break,
                    Some(UdpPollOutcome::Processed { .. }) => {
                        if next_playout.is_none() {
                            next_playout = Some(Instant::now() + Duration::from_millis(60));
                        }
                    }
                }
            }
            let now = Instant::now();
            if let Some(due) = next_playout.filter(|due| now >= *due) {
                if metrics.snapshot().jitter_frames > 0
                    && matches!(pipeline.decode_next()?, DecodeStep::Decoded { .. })
                {
                    decoded += 1;
                }
                if let Some(pcm) = &mut consumer {
                    let mut samples = [0_f32; SAMPLES];
                    pcm.pop_into(&mut samples);
                    for sample in samples {
                        peak = peak.max(sample.abs());
                    }
                }
                next_playout = Some(next_playout_deadline(due, now));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_datagrams_do_not_end_the_receiver_or_advance_replay() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([0x35; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let metrics = Arc::new(StreamCounters::default());
        let (producer, _consumer) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
        let worker = StreamDecodeWorker::new_for_opus(
            ObservedDecoder {
                decoder: lab_decoder::Decoder::new().unwrap(),
                peak: Arc::new(AtomicU32::new(0)),
            },
            producer,
            0,
            OpusFrameConfig::new(48_000, 2, 20).unwrap(),
        )
        .unwrap();
        let mut receiver = Receiver::bind_with_worker(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            worker,
            Arc::clone(&metrics),
        )
        .unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = receiver.local_addr().unwrap();
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25_f32; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let valid = sealer.seal(0, 0, &payload[..length]).unwrap();
        let mut tampered = valid.datagram.clone();
        *tampered.last_mut().unwrap() ^= 1;
        for packet in [&[0_u8][..], &[0_u8; 1_201][..], &tampered[..]] {
            sender.send_to(packet, address).unwrap();
        }
        sender.send_to(&valid.datagram, address).unwrap();
        sender.send_to(&valid.datagram, address).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().rejected_packets < 4 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
            std::thread::yield_now();
        }
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.rejected_packets, 4);
        assert_eq!(snapshot.jitter_frames, 1);
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 0, .. }
        ));
    }

    #[test]
    fn playout_deadline_does_not_accumulate_poll_delay() {
        let origin = Instant::now();
        let mut deadline = origin;
        for step in 1..=1_000 {
            let now = deadline + Duration::from_millis(2);
            deadline = next_playout_deadline(deadline, now);
            assert_eq!(deadline, origin + PLAYOUT_PERIOD * step);
        }
    }

    #[test]
    fn playout_deadline_rebases_after_a_missed_period() {
        let deadline = Instant::now();
        let late = deadline + PLAYOUT_PERIOD;
        assert_eq!(next_playout_deadline(deadline, late), late + PLAYOUT_PERIOD);
        let much_later = deadline + Duration::from_secs(5);
        assert_eq!(
            next_playout_deadline(deadline, much_later),
            much_later + PLAYOUT_PERIOD
        );
    }

    #[test]
    fn playback_arguments_preserve_default_and_require_explicit_selection() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>()
        };
        let defaults = Options::parse(args(&[])).unwrap();
        assert!(!defaults.play && !defaults.require_tone);
        let playback =
            Options::parse(args(&["--require-tone", "--play", "--select-output"])).unwrap();
        assert!(playback.play && playback.require_tone && playback.select_output);
        assert!(Options::parse(args(&["--select-output"])).is_err());
        assert!(Options::parse(args(&["--play", "--play"])).is_err());
    }

    #[test]
    fn decoder_observes_the_decoded_tone() {
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let samples: Vec<f32> = (0..SAMPLES)
            .map(|index| if (index / 48) % 2 == 0 { 0.25 } else { -0.25 })
            .collect();
        let mut packet = [0_u8; 1_148];
        let length = encoder.encode_float(&samples, &mut packet).unwrap();
        let peak = Arc::new(AtomicU32::new(0));
        let mut decoder = ObservedDecoder {
            decoder: lab_decoder::Decoder::new().unwrap(),
            peak: Arc::clone(&peak),
        };
        let mut pcm = [0.0; SAMPLES];
        assert_eq!(
            decoder.decode(&packet[..length], &mut pcm).unwrap(),
            SAMPLES
        );
        assert!(f32::from_bits(peak.load(Ordering::Relaxed)) > 0.001);
    }
}
