//! Paired Android-to-desktop UDP audio lab.
//! The fixed ID and secret are public test data, never product credentials.
#[cfg(all(windows, feature = "playback"))]
#[allow(dead_code)]
mod common;
#[cfg(any(test, all(windows, feature = "playback")))]
mod lab_control;
mod lab_decoder;

use std::fmt::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
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
use sonimesh_diagnostics::{StreamCounters, StreamMetricsSnapshot};
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
    let mut playback = Playback::prepare(options.play, options.select_output)?;
    if options.control {
        playback.start_controls()?;
    }
    let listener = PairingListener::bind(SocketAddrV4::new(options.bind, 47610))?;
    let address = listener.local_addr()?;
    let key = generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let pin = key.key_pair.public_key_der();
    let mut private = Zeroizing::new(key.key_pair.serialize_der());
    let tls = Arc::new(server_config(
        key.cert.der().clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private.to_vec())),
    )?);
    private.zeroize();
    let mut pin_hex = String::with_capacity(pin.len() * 2);
    for byte in &pin {
        write!(pin_hex, "{byte:02x}")?;
    }
    println!("SPKI_HEX {pin_hex}");
    println!("LISTENING {address}");
    println!("INVITATION_HOST {}", options.host);
    loop {
        println!("READY");
        let started = Instant::now();
        let stream = loop {
            if let Some(stream) = listener.try_accept()? {
                break stream;
            }
            if !options.keep_listening && started.elapsed() >= Duration::from_secs(90) {
                return Err("client connection timed out".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let result = run_session(stream, Arc::clone(&tls), &pin, &options, &mut playback);
        playback.abort();
        if !options.keep_listening {
            return result;
        }
        match result {
            Ok(()) => println!("RESULT session complete"),
            Err(error) => eprintln!("Session ended: {error}"),
        }
    }
}

fn run_session(
    stream: std::net::TcpStream,
    tls: Arc<rustls::ServerConfig>,
    pin: &[u8],
    options: &Options,
    playback: &mut Playback,
) -> LabResult<()> {
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
            control_host: &options.host.to_string(),
            control_port: 47610,
            desktop_tls_spki: pin,
            expires_at_ms: 120_100,
        },
        &mut payload,
    )?;
    payload.truncate(length);
    let (channel, root) = accept_pairing_stream(
        stream,
        tls,
        &mut pairing,
        &mut session,
        100,
        PairingSource::new([0x72; 16]),
        &InvitationInputs {
            invitation: id,
            pairing_secret: LAB_SECRET,
            scanned_payload: &payload,
        },
    )?;
    println!("RESULT pairing verified");
    negotiate_hello(channel, session, root, options, playback)
}

#[allow(clippy::struct_excessive_bools)] // Independent command-line switches.
struct Options {
    require_tone: bool,
    play: bool,
    select_output: bool,
    keep_listening: bool,
    control: bool,
    status: bool,
    bind: Ipv4Addr,
    host: Ipv4Addr,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            require_tone: false,
            play: false,
            select_output: false,
            keep_listening: false,
            control: false,
            status: false,
            bind: Ipv4Addr::LOCALHOST,
            host: Ipv4Addr::LOCALHOST,
        }
    }
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> LabResult<Self> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        let mut bind_set = false;
        let mut host_set = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--require-tone" if !result.require_tone => result.require_tone = true,
                "--play" if !result.play => result.play = true,
                "--select-output" if !result.select_output => result.select_output = true,
                "--keep-listening" if !result.keep_listening => result.keep_listening = true,
                "--control" if !result.control => result.control = true,
                "--status" if !result.status => result.status = true,
                "--bind" if !bind_set => {
                    result.bind = args
                        .next()
                        .ok_or("--bind requires an IPv4 address")?
                        .parse()?;
                    bind_set = true;
                }
                "--host" if !host_set => {
                    result.host = args
                        .next()
                        .ok_or("--host requires an IPv4 address")?
                        .parse()?;
                    host_set = true;
                }
                _ => {
                    return Err(
                        "usage: paired-audio-lab [--require-tone] [--play [--select-output] [--control]] [--status] [--keep-listening] [--bind IPv4 --host IPv4]"
                            .into(),
                    );
                }
            }
        }
        if result.select_output && !result.play {
            return Err("--select-output requires --play".into());
        }
        if result.control && !result.play {
            return Err("--control requires --play".into());
        }
        if bind_set != host_set
            || result.host.is_unspecified()
            || result.host.is_multicast()
            || result.host.is_broadcast()
        {
            return Err("provide both --bind and a unicast --host IPv4 address".into());
        }
        Ok(result)
    }
}

fn negotiate_hello(
    channel: sonimesh_app::TrustedControlChannel,
    session: SessionState,
    root: SessionSecret,
    options: &Options,
    playback: &mut Playback,
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
        options,
        playback,
    )
}

const SAMPLES: usize = 1_920;
const PLAYOUT_PERIOD: Duration = Duration::from_millis(20);
const STATUS_PERIOD: Duration = Duration::from_secs(5);
type Receiver = UdpReceiverPipeline<ObservedDecoder, 8, 1_148, SAMPLES>;

fn status_if_due(
    next: &mut Option<Instant>,
    now: Instant,
    metrics: StreamMetricsSnapshot,
    decoded: u64,
    peak: f32,
    output: &str,
) -> Option<String> {
    if next.is_none_or(|due| now < due) {
        return None;
    }
    *next = Some(now + STATUS_PERIOD);
    Some(format!(
        "STATUS packets {} rejected {} decoded {decoded} peak {peak:.3} plc {} decode_errors {} buffered {} output {output}",
        metrics.authenticated_packets,
        metrics.rejected_packets,
        metrics.plc_frames,
        metrics.decode_errors,
        metrics.jitter_frames,
    ))
}

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

fn decode_playout(receiver: &mut Receiver) -> LabResult<Option<DecodeStep>> {
    match receiver.decode_next() {
        Ok(step) => Ok(Some(step)),
        Err(DecodeWorkerError::Decoder(error))
            if error.code() == opus::ErrorCode::InvalidPacket =>
        {
            // The pipeline has already retired the affected audio state.
            Ok(None)
        }
        Err(error) => Err(error.into()),
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
    fn start_controls(&self) -> LabResult<()> {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, _)) = &self.0 {
            lab_control::start_console_control(controller.gain_handle());
            return Ok(());
        }
        Err("console control requires Windows playback".into())
    }

    fn abort(&mut self) {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, _)) = &mut self.0 {
            controller.stop();
        }
    }

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

    fn monitor(&mut self, receiver: &mut Receiver, retry_at: &mut Option<Instant>) -> bool {
        #[cfg(all(windows, feature = "playback"))]
        if let Some((controller, _)) = &mut self.0 {
            return maintain_output(controller, receiver, retry_at, Instant::now());
        }
        let _ = (receiver, retry_at);
        false
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

#[cfg(any(test, all(windows, feature = "playback")))]
fn maintain_output<O: sonimesh_platform::AudioOutput>(
    output: &mut sonimesh_app::ReceiverOutputController<O, SAMPLES>,
    receiver: &mut Receiver,
    retry_at: &mut Option<Instant>,
    now: Instant,
) -> bool {
    if let Some(health) = output.monitor() {
        eprintln!("Output failed ({health:?}); retrying the selected device.");
        *retry_at = Some(now + Duration::from_secs(1));
    }
    if retry_at.is_some_and(|deadline| now >= deadline) {
        match output.recover(receiver) {
            Ok(()) => {
                *retry_at = None;
                println!("Output recovered; waiting for fresh audio.");
                return true;
            }
            Err(error) => {
                eprintln!("Output recovery failed: {error}");
                *retry_at = Some(now + Duration::from_secs(1));
            }
        }
    }
    retry_at.is_some()
}

#[allow(clippy::too_many_lines)] // This one-shot lab owns the control and media lifetimes together.
fn receive_audio<S: std::io::Read + std::io::Write>(
    framed: &mut UntrustedControlStream<S>,
    coordinator: &mut ControlStreamCoordinator,
    control_token: sonimesh_app::ControlSessionToken,
    root: SessionSecret,
    options: &Options,
    playback: &mut Playback,
) -> LabResult<()> {
    let mut allocator = MediaSessionAllocator::new(root);
    let mut receiver: Option<Receiver> = None;
    let mut consumer = None;
    let mut counters = None;
    let mut next_playout = None;
    let mut next_status = None;
    let mut retry_at = None;
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
                                SocketAddrV4::new(options.bind, 39093).into(),
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
                            next_status = options.status.then(|| Instant::now() + STATUS_PERIOD);
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
                                "RESULT packets {} rejected {} late {} resync_dropped {} plc {} decode_errors {} buffered {}",
                                snapshot.authenticated_packets,
                                snapshot.rejected_packets,
                                snapshot.late_packets,
                                snapshot.jitter_resync_dropped_frames,
                                snapshot.plc_frames,
                                snapshot.decode_errors,
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
                        if retry_at.is_some() {
                            return Err(
                                "output device did not recover before the session ended".into()
                            );
                        }
                        output_result?;
                        if playing {
                            peak = f32::from_bits(decoded_peak.load(Ordering::Relaxed));
                        }
                        println!("RESULT decoded {decoded} peak {peak:.3}");
                        if decoded == 0 || peak < 0.001 {
                            if options.require_tone {
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
        if let (Some(pipeline), Some(metrics)) = (&mut receiver, &counters) {
            if playback.monitor(pipeline, &mut retry_at) {
                next_playout = None;
            }
            for _ in 0..32 {
                match poll_media(pipeline)? {
                    None => continue,
                    Some(UdpPollOutcome::Empty) => break,
                    Some(UdpPollOutcome::Processed { .. }) => {
                        if next_playout.is_none() && retry_at.is_none() {
                            next_playout = Some(Instant::now() + Duration::from_millis(60));
                        }
                    }
                }
            }
            let now = Instant::now();
            if let Some(due) = next_playout.filter(|due| now >= *due) {
                if metrics.snapshot().jitter_frames > 0 {
                    match decode_playout(pipeline)? {
                        Some(DecodeStep::Decoded { .. }) => decoded += 1,
                        Some(_) => {}
                        None => {
                            next_playout = None;
                            continue;
                        }
                    }
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
            if next_status.is_some_and(|due| now >= due) {
                let output = if retry_at.is_some() {
                    "recovering"
                } else if playing {
                    "active"
                } else {
                    "software-check"
                };
                let decoded_peak = f32::from_bits(decoded_peak.load(Ordering::Relaxed));
                if let Some(status) = status_if_due(
                    &mut next_status,
                    now,
                    metrics.snapshot(),
                    decoded,
                    decoded_peak,
                    output,
                ) {
                    println!("{status}");
                }
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
    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
        RenderSource, SampleFormat,
    };

    #[derive(Default)]
    struct FaultOutput {
        source: Option<Box<dyn RenderSource>>,
        failed: bool,
        reject_start: bool,
        starts: usize,
    }

    impl AudioOutput for FaultOutput {
        fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
            Ok(Vec::new())
        }

        fn start(
            &mut self,
            _: Option<&OutputDeviceId>,
            _: OutputConfig,
            source: Box<dyn RenderSource>,
        ) -> Result<(), OutputError> {
            self.starts += 1;
            if self.reject_start {
                return Err(OutputError::DeviceUnavailable);
            }
            self.failed = false;
            self.source = Some(source);
            Ok(())
        }

        fn stop(&mut self) {
            self.source = None;
        }

        fn health(&self) -> OutputHealth {
            if self.failed {
                OutputHealth::StreamError
            } else {
                OutputHealth::Healthy
            }
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Exercises fault, retry, and fresh media on one receiver.
    fn output_retries_without_replacing_media_session_and_discards_stale_audio() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let metrics = Arc::new(StreamCounters::default());
        let (producer, _) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
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
        let factory = sonimesh_app::ReceiverPcmEndpointFactory::new(
            8,
            sonimesh_app::SourcePcmFormat::new(48_000, 2).unwrap(),
            Arc::clone(&metrics),
        );
        let mut output =
            sonimesh_app::ReceiverOutputController::new(FaultOutput::default(), factory);
        output
            .start(
                sonimesh_app::OutputSelection {
                    device: None,
                    config: OutputConfig {
                        sample_rate_hz: 48_000,
                        channels: 2,
                        sample_format: SampleFormat::Float32,
                        requested_callback_frames: None,
                        maximum_callback_frames: 960,
                    },
                },
                &mut receiver,
            )
            .unwrap();
        let controls = output.gain_handle();
        lab_control::apply_control("volume 50", &controls).unwrap();
        lab_control::apply_control("mute", &controls).unwrap();
        output.output_mut().failed = true;
        output.output_mut().reject_start = true;
        let now = Instant::now();
        let mut retry = None;
        assert!(maintain_output(&mut output, &mut receiver, &mut retry, now));
        assert!(maintain_output(
            &mut output,
            &mut receiver,
            &mut retry,
            now + Duration::from_millis(999)
        ));
        assert_eq!(output.output().starts, 1);
        assert!(maintain_output(
            &mut output,
            &mut receiver,
            &mut retry,
            now + Duration::from_secs(1)
        ));
        assert_eq!(output.output().starts, 2);

        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = receiver.local_addr().unwrap();
        let first = sealer.seal(0, 0, &payload[..length]).unwrap();
        sender.send_to(&first.datagram, address).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 1 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert_eq!(metrics.snapshot().jitter_frames, 1);
        output.output_mut().reject_start = false;
        assert!(maintain_output(
            &mut output,
            &mut receiver,
            &mut retry,
            now + Duration::from_secs(2)
        ));
        assert!(retry.is_none());
        assert_eq!(output.gain_handle().snapshot(), controls.snapshot());
        assert_eq!(metrics.snapshot().jitter_frames, 0);
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::NotReady
        ));
        let second = sealer.seal(0, 960, &payload[..length]).unwrap();
        sender.send_to(&second.datagram, address).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 2 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 1, .. }
        ));
        let mut samples = [0.0; SAMPLES];
        output
            .output_mut()
            .source
            .as_mut()
            .unwrap()
            .render(&mut samples);
        assert!(samples.iter().all(|sample| sample.abs() < 0.001));
        lab_control::apply_control("unmute", &controls).unwrap();
        let third = sealer.seal(0, 1_920, &payload[..length]).unwrap();
        sender.send_to(&third.datagram, address).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 3 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 2, .. }
        ));
        output
            .output_mut()
            .source
            .as_mut()
            .unwrap()
            .render(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
        lab_control::apply_control("mute", &controls).unwrap();
        output.stop();
        output
            .start(
                sonimesh_app::OutputSelection {
                    device: None,
                    config: OutputConfig {
                        sample_rate_hz: 48_000,
                        channels: 2,
                        sample_format: SampleFormat::Float32,
                        requested_callback_frames: None,
                        maximum_callback_frames: 960,
                    },
                },
                &mut receiver,
            )
            .unwrap();
        assert_eq!(output.gain_handle().snapshot(), controls.snapshot());
        let fourth = sealer.seal(0, 2_880, &payload[..length]).unwrap();
        sender.send_to(&fourth.datagram, address).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 4 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: 3, .. }
        ));
        output
            .output_mut()
            .source
            .as_mut()
            .unwrap()
            .render(&mut samples);
        assert!(samples.iter().all(|sample| sample.abs() < 0.001));
    }

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
    fn invalid_opus_frame_resets_playout_and_accepts_fresh_audio() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([0x35; 32]);
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let metrics = Arc::new(StreamCounters::default());
        let (producer, mut consumer) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
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
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let invalid = sealer.seal(0, 0, &[0xff]).unwrap();
        sender
            .send_to(&invalid.datagram, receiver.local_addr().unwrap())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 1 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert!(decode_playout(&mut receiver).unwrap().is_none());
        assert_eq!(metrics.snapshot().decode_errors, 1);

        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let valid = sealer.seal(0, 960, &payload[..length]).unwrap();
        sender
            .send_to(&valid.datagram, receiver.local_addr().unwrap())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while metrics.snapshot().authenticated_packets < 2 && Instant::now() < until {
            poll_media(&mut receiver).unwrap();
        }
        assert!(matches!(
            decode_playout(&mut receiver).unwrap(),
            Some(DecodeStep::Decoded { sequence: 1, .. })
        ));
        let mut samples = [0.0; SAMPLES];
        consumer.pop_into(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
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
        assert!(!defaults.status);
        assert!(Options::parse(args(&["--status"])).unwrap().status);
        assert!(Options::parse(args(&["--status", "--status"])).is_err());
        let playback =
            Options::parse(args(&["--require-tone", "--play", "--select-output"])).unwrap();
        assert!(playback.play && playback.require_tone && playback.select_output);
        assert!(Options::parse(args(&["--select-output"])).is_err());
        assert!(Options::parse(args(&["--control"])).is_err());
        assert!(
            Options::parse(args(&["--play", "--control"]))
                .unwrap()
                .control
        );
        assert!(Options::parse(args(&["--play", "--play"])).is_err());
        assert!(
            Options::parse(args(&["--keep-listening"]))
                .unwrap()
                .keep_listening
        );
        assert!(Options::parse(args(&["--keep-listening", "--keep-listening"])).is_err());
        let network = Options::parse(args(&["--bind", "0.0.0.0", "--host", "10.0.2.2"])).unwrap();
        assert_eq!(network.bind, Ipv4Addr::UNSPECIFIED);
        assert_eq!(network.host, Ipv4Addr::new(10, 0, 2, 2));
        assert!(Options::parse(args(&["--bind", "0.0.0.0"])).is_err());
        assert!(Options::parse(args(&["--bind", "0.0.0.0", "--host", "0.0.0.0"])).is_err());
    }

    #[test]
    fn live_status_reports_media_progress_and_output_recovery() {
        let now = Instant::now();
        let mut next = Some(now + STATUS_PERIOD);
        let metrics = StreamMetricsSnapshot {
            authenticated_packets: 120,
            rejected_packets: 3,
            plc_frames: 2,
            decode_errors: 1,
            jitter_frames: 4,
            ..StreamMetricsSnapshot::default()
        };
        assert!(status_if_due(&mut next, now, metrics, 115, 0.25, "recovering").is_none());
        let late = now + STATUS_PERIOD * 3;
        assert_eq!(
            status_if_due(&mut next, late, metrics, 115, 0.25, "recovering").as_deref(),
            Some(
                "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output recovering"
            )
        );
        assert!(status_if_due(&mut next, late, metrics, 115, 0.25, "recovering").is_none());
        assert_eq!(
            status_if_due(
                &mut next,
                late + STATUS_PERIOD,
                StreamMetricsSnapshot::default(),
                0,
                0.0,
                "software-check"
            )
            .as_deref(),
            Some(
                "STATUS packets 0 rejected 0 decoded 0 peak 0.000 plc 0 decode_errors 0 buffered 0 output software-check"
            )
        );
        assert!(status_if_due(&mut None, late, metrics, 0, 0.0, "active").is_none());
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
