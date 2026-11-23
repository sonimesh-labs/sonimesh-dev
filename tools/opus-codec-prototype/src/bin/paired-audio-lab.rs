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
    AdaptiveTargetConfig, ControlReply, ControlStreamAction, ControlStreamCoordinator,
    InvitationInputs, MediaSessionAllocator, MediaStreamIdentity, PairingId,
    PairingInvitationController, PairingListener, PairingQrInvitation, PairingSource,
    ReceiverPipelineError, SessionState, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline,
    accept_pairing_stream, begin_from_rendered_payload,
};
use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, InsertOutcome, JitterMode, OpusFrameConfig,
    RealtimePcmConsumer, StreamDecodeWorker, TargetGatedPlayoutConfig, realtime_pcm_queue,
};
use sonimesh_diagnostics::{StreamCounters, StreamMetricsSnapshot};
use sonimesh_network::{
    IngressError, UdpReceiveError, UntrustedControlPoll, UntrustedControlStream,
    UntrustedControlWrite, server_config,
};
use sonimesh_protocol::{
    Capability, Envelope, Hello, MAX_QR_PAYLOAD_LEN, MediaDirection, PairingQrPayload,
    SessionSecret, StreamState, envelope,
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
    let mut playback = Playback::prepare(&options)?;
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
    let invitation = render_lab_invitation(pin.as_ref(), options.host)?;
    println!("SPKI_HEX {pin_hex}");
    println!("INVITATION_HEX {}", bytes_to_hex(&invitation));
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
        println!("RESULT client connected");
        let result = run_session(stream, Arc::clone(&tls), &pin, &options, &mut playback);
        playback.abort();
        if !options.keep_listening {
            return result;
        }
        match result {
            Ok(()) => println!("RESULT session complete"),
            Err(error) => {
                println!("RESULT session failed");
                eprintln!("Session ended: {error}");
            }
        }
    }
}

fn render_lab_invitation(pin: &[u8], host: Ipv4Addr) -> LabResult<Vec<u8>> {
    let host = host.to_string();
    let payload = PairingQrPayload {
        protocol_version: u8::try_from(sonimesh_protocol::CONTROL_PROTOCOL_VERSION)?,
        pairing_id: LAB_ID,
        pairing_secret: LAB_SECRET,
        control_host: &host,
        control_port: 47610,
        desktop_tls_spki: pin,
        expires_at_ms: 120_100,
    };
    let mut encoded = vec![0_u8; MAX_QR_PAYLOAD_LEN];
    let length = payload.encode_into(&mut encoded)?;
    encoded.truncate(length);
    Ok(encoded)
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
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
    output_id_stdin: bool,
    output_id_framed: bool,
    keep_listening: bool,
    control: bool,
    status: bool,
    adaptive: bool,
    startup_buffer: Duration,
    bind: Ipv4Addr,
    host: Ipv4Addr,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            require_tone: false,
            play: false,
            select_output: false,
            output_id_stdin: false,
            output_id_framed: false,
            keep_listening: false,
            control: false,
            status: false,
            adaptive: false,
            startup_buffer: Duration::from_millis(60),
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
        let mut startup_buffer_set = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--require-tone" if !result.require_tone => result.require_tone = true,
                "--play" if !result.play => result.play = true,
                "--select-output" if !result.select_output => result.select_output = true,
                "--output-id-stdin" if !result.output_id_stdin => result.output_id_stdin = true,
                "--output-id-framed" if !result.output_id_framed => result.output_id_framed = true,
                "--keep-listening" if !result.keep_listening => result.keep_listening = true,
                "--control" if !result.control => result.control = true,
                "--status" if !result.status => result.status = true,
                "--adaptive" if !result.adaptive => result.adaptive = true,
                "--startup-buffer-ms" if !startup_buffer_set => {
                    let value = args
                        .next()
                        .ok_or("--startup-buffer-ms requires a value")?
                        .parse::<u64>()
                        .map_err(|_| "startup buffer must be 20 to 120 ms in 20 ms steps")?;
                    if !(20..=120).contains(&value) || value % 20 != 0 {
                        return Err("startup buffer must be 20 to 120 ms in 20 ms steps".into());
                    }
                    result.startup_buffer = Duration::from_millis(value);
                    startup_buffer_set = true;
                }
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
                        "usage: paired-audio-lab [--require-tone] [--play [--select-output | --output-id-stdin | --output-id-framed] [--control]] [--status] [--adaptive] [--startup-buffer-ms 20-120] [--keep-listening] [--bind IPv4 --host IPv4]"
                            .into(),
                    );
                }
            }
        }
        if (result.select_output || result.output_id_stdin || result.output_id_framed)
            && !result.play
        {
            return Err("output selection requires --play".into());
        }
        if u8::from(result.select_output)
            + u8::from(result.output_id_stdin)
            + u8::from(result.output_id_framed)
            > 1
        {
            return Err("choose one output selection method".into());
        }
        if result.output_id_stdin && result.control {
            return Err("--output-id-stdin cannot be combined with --control".into());
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
const STOP_DRAIN_QUIET: Duration = Duration::from_millis(160);
const STOP_DRAIN_LIMIT: Duration = Duration::from_millis(500);
const MAX_CONSECUTIVE_PLC: u8 = 3;
const MAX_MEDIA_POLLS: usize = 32;
const PLAYOUT_STALL: Duration = Duration::from_millis(160);
const MEDIA_IDLE: Duration = Duration::from_secs(2);
type Receiver = UdpReceiverPipeline<ObservedDecoder, 8, 1_148, SAMPLES>;
type PreparedReceiver = (Receiver, RealtimePcmConsumer<SAMPLES>, Arc<StreamCounters>);

fn start_prepared<T, E>(
    pending: &mut Option<(MediaStreamIdentity, T)>,
    identity: MediaStreamIdentity,
    prepare: impl FnOnce() -> Result<T, E>,
    start: impl FnOnce(&mut T) -> Result<bool, E>,
) -> Result<Result<(T, bool), E>, E> {
    let mut prepared = match pending.take() {
        Some((previous, prepared)) if previous == identity => prepared,
        _ => prepare()?,
    };
    match start(&mut prepared) {
        Ok(playing) => Ok(Ok((prepared, playing))),
        Err(error) => {
            *pending = Some((identity, prepared));
            Ok(Err(error))
        }
    }
}

#[derive(Default)]
struct ScheduleObservations {
    decisions: u64,
    frame_late: u64,
    max_lateness_us: u64,
}

#[derive(Default)]
struct MediaActivity {
    packets: u64,
    last_packet: Option<Instant>,
}

impl MediaActivity {
    fn observe(&mut self, now: Instant, packets: u64) {
        if packets > self.packets {
            self.last_packet = Some(now);
        }
        self.packets = packets;
    }

    fn status(&self, now: Instant) -> &'static str {
        match self.last_packet {
            None => "waiting",
            Some(last) if now.duration_since(last) < MEDIA_IDLE => "receiving",
            Some(_) => "interrupted",
        }
    }
}

impl ScheduleObservations {
    fn observe(&mut self, due: Instant, now: Instant) {
        let lateness = now.duration_since(due);
        self.decisions = self.decisions.saturating_add(1);
        if lateness >= PLAYOUT_PERIOD {
            self.frame_late = self.frame_late.saturating_add(1);
        }
        let micros = u64::try_from(lateness.as_micros()).unwrap_or(u64::MAX);
        self.max_lateness_us = self.max_lateness_us.max(micros);
    }
}

#[derive(Clone, Copy)]
struct ResyncObservation {
    dropped: usize,
    loop_gap: Option<Duration>,
    previous_decision: Option<(Duration, Duration)>,
}

#[derive(Default)]
struct ResyncObservations {
    total: u64,
    first: [Option<ResyncObservation>; 8],
}

impl ResyncObservations {
    fn observe_admission(
        &mut self,
        admission: InsertOutcome,
        loop_gap: Option<Duration>,
        previous_decision: Option<(Instant, Duration)>,
        now: Instant,
    ) {
        if let InsertOutcome::Resynchronized { dropped_frames } = admission {
            self.record(ResyncObservation {
                dropped: dropped_frames,
                loop_gap,
                previous_decision: previous_decision
                    .map(|(when, lateness)| (now.duration_since(when), lateness)),
            });
        }
    }

    fn record(&mut self, observation: ResyncObservation) {
        if let Some(slot) = usize::try_from(self.total)
            .ok()
            .and_then(|index| self.first.get_mut(index))
        {
            *slot = Some(observation);
        }
        self.total = self.total.saturating_add(1);
    }

    fn print(&self) {
        println!("RESULT resync events {}", self.total);
        for (index, event) in self.first.iter().flatten().enumerate() {
            let loop_gap = event
                .loop_gap
                .map_or_else(|| "none".to_owned(), |gap| gap.as_micros().to_string());
            let (age, lateness) = event.previous_decision.map_or_else(
                || ("none".to_owned(), "none".to_owned()),
                |(age, lateness)| {
                    (
                        age.as_micros().to_string(),
                        lateness.as_micros().to_string(),
                    )
                },
            );
            println!(
                "RESULT resync event {} dropped {} loop_gap_us {loop_gap} decision_age_us {age} decision_lateness_us {lateness}",
                index + 1,
                event.dropped,
            );
        }
    }
}

fn status_if_due(
    next: &mut Option<Instant>,
    now: Instant,
    metrics: StreamMetricsSnapshot,
    decoded: u64,
    peak: f32,
    output: &str,
    adaptive: bool,
) -> Option<String> {
    if next.is_none_or(|due| now < due) {
        return None;
    }
    *next = Some(now + STATUS_PERIOD);
    Some(format_status(metrics, decoded, peak, output, adaptive))
}

fn format_status(
    metrics: StreamMetricsSnapshot,
    decoded: u64,
    peak: f32,
    output: &str,
    adaptive: bool,
) -> String {
    let mut status = format!(
        "STATUS packets {} rejected {} decoded {decoded} peak {peak:.3} plc {} decode_errors {} buffered {} output {output} late {} resync_dropped {}",
        metrics.authenticated_packets,
        metrics.rejected_packets,
        metrics.plc_frames,
        metrics.decode_errors,
        metrics.jitter_frames,
        metrics.late_packets,
        metrics.jitter_resync_dropped_frames,
    );
    if adaptive {
        write!(
            status,
            " playout adaptive target {} min {} max {} jitter_ms {} gated {} forced {} resets {}",
            metrics.target_frames,
            metrics.target_min_frames,
            metrics.target_max_frames,
            metrics.target_smoothed_jitter_ms,
            metrics.target_gated_ticks,
            metrics.target_forced_playouts,
            metrics.stream_resets
        )
        .expect("writing to a String cannot fail");
    } else {
        status.push_str(" playout fixed");
    }
    status
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

fn drain_stale_media(receiver: &mut Receiver) -> LabResult<bool> {
    for _ in 0..MAX_MEDIA_POLLS {
        if matches!(poll_media(receiver)?, Some(UdpPollOutcome::Empty)) {
            receiver.hard_resynchronize();
            return Ok(true);
        }
    }
    Ok(false)
}

fn playout_stalled(active: bool, gap: Option<Duration>) -> bool {
    active && gap.is_some_and(|elapsed| elapsed >= PLAYOUT_STALL)
}

fn next_playout_deadline(deadline: Instant, now: Instant) -> Instant {
    let phase = now.duration_since(deadline).as_nanos() % PLAYOUT_PERIOD.as_nanos();
    let remaining_nanos = u64::try_from(PLAYOUT_PERIOD.as_nanos() - phase)
        .expect("one playout period fits in u64 nanoseconds");
    now + Duration::from_nanos(remaining_nanos).max(PLAYOUT_PERIOD / 2)
}

fn decode_playout(
    receiver: &mut Receiver,
    arrivals_pending: bool,
) -> LabResult<Option<DecodeStep>> {
    match receiver.decode_next_with_arrivals_pending(arrivals_pending) {
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

fn decode_live_playout(
    receiver: &mut Receiver,
    metrics: &StreamCounters,
    consecutive_plc: &mut u8,
) -> LabResult<Option<DecodeStep>> {
    if *consecutive_plc >= MAX_CONSECUTIVE_PLC && metrics.snapshot().jitter_frames == 0 {
        receiver.hard_resynchronize();
        *consecutive_plc = 0;
        return Ok(None);
    }
    let step = decode_playout(receiver, true)?;
    match step {
        Some(DecodeStep::Concealed { .. }) => {
            *consecutive_plc = consecutive_plc.saturating_add(1);
        }
        Some(DecodeStep::NotReady) => {}
        _ => *consecutive_plc = 0,
    }
    Ok(step)
}

fn drain_stop_tail(
    receiver: &mut Receiver,
    metrics: &StreamCounters,
    consumer: &mut Option<RealtimePcmConsumer<SAMPLES>>,
    next_playout: Option<Instant>,
    decoded: &mut u64,
    peak: &mut f32,
) -> LabResult<()> {
    let start = Instant::now();
    let deadline = start + STOP_DRAIN_LIMIT;
    let mut last_activity = start;
    let mut next = next_playout.unwrap_or(start);
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        for _ in 0..32 {
            match poll_media(receiver)? {
                Some(UdpPollOutcome::Empty) => break,
                Some(UdpPollOutcome::Processed { .. }) => last_activity = Instant::now(),
                None => {}
            }
        }
        let now = Instant::now();
        if now >= next && metrics.snapshot().jitter_frames > 0 {
            if let Some(DecodeStep::Decoded { .. }) = decode_playout(receiver, false)? {
                *decoded += 1;
                last_activity = now;
            }
            if let Some(pcm) = consumer {
                let mut samples = [0_f32; SAMPLES];
                pcm.pop_into(&mut samples);
                for sample in samples {
                    *peak = peak.max(sample.abs());
                }
            }
            next = next_playout_deadline(next, now);
        }
        if metrics.snapshot().jitter_frames == 0
            && now.duration_since(last_activity) >= STOP_DRAIN_QUIET
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
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

    fn prepare(options: &Options) -> LabResult<Self> {
        #[cfg(all(windows, feature = "playback"))]
        {
            if options.play {
                let (output, selection) = if options.output_id_stdin || options.output_id_framed {
                    use sonimesh_platform::{CpalOutput, OutputDeviceId};
                    use std::io::Read;
                    let mut id = String::new();
                    if options.output_id_framed {
                        let mut stdin = std::io::stdin().lock();
                        let mut length = [0_u8; 4];
                        stdin.read_exact(&mut length)?;
                        let length = u32::from_be_bytes(length) as usize;
                        if length == 0 || length > 4096 {
                            return Err("invalid output ID".into());
                        }
                        let mut bytes = vec![0; length];
                        stdin.read_exact(&mut bytes)?;
                        id = String::from_utf8(bytes)?;
                    } else {
                        std::io::stdin().take(4097).read_to_string(&mut id)?;
                    }
                    if id.is_empty() || id.len() > 4096 {
                        return Err("invalid output ID".into());
                    }
                    let output = CpalOutput::new();
                    let probe = output.device_probe(&OutputDeviceId::new(id))?;
                    let config = probe.output_config(None, 8_192)?;
                    (
                        output,
                        sonimesh_app::OutputSelection {
                            device: Some(probe.device.id),
                            config,
                        },
                    )
                } else {
                    common::prepare_output(options.select_output).map_err(|e| e.to_string())?
                };
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
            if options.play {
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
    let mut pending_receiver: Option<(MediaStreamIdentity, PreparedReceiver)> = None;
    let mut consumer = None;
    let mut counters = None;
    let mut next_playout = None;
    let mut consecutive_plc = 0;
    let mut next_status = None;
    let mut retry_at = None;
    let mut decoded = 0_u64;
    let mut peak = 0_f32;
    let mut schedule = ScheduleObservations::default();
    let mut activity = MediaActivity::default();
    let mut resyncs = ResyncObservations::default();
    let mut previous_loop = None;
    let mut draining_stale = false;
    let mut previous_decision: Option<(Instant, Duration)> = None;
    let mut playing = false;
    let decoded_peak = Arc::new(AtomicU32::new(0));
    let deadline = Instant::now() + Duration::from_secs(40);
    while receiver.is_some() || Instant::now() < deadline {
        let loop_start = Instant::now();
        let loop_gap = previous_loop.map(|previous| loop_start.duration_since(previous));
        previous_loop = Some(loop_start);
        if !draining_stale && playout_stalled(next_playout.is_some(), loop_gap) {
            if let Some(pipeline) = &mut receiver {
                pipeline.hard_resynchronize();
                next_playout = None;
                consecutive_plc = 0;
                draining_stale = true;
            }
        }
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
                            let prepared = start_prepared(
                                &mut pending_receiver,
                                request.identity,
                                || {
                                    let grant = allocator
                                        .supply_for_authenticated_control_session(
                                            coordinator,
                                            control_token,
                                            request.identity,
                                            MediaDirection::AndroidToDesktop,
                                        )?;
                                    let metrics = Arc::new(StreamCounters::default());
                                    let (producer, next_consumer) =
                                        realtime_pcm_queue::<SAMPLES>(8)?;
                                    let worker = StreamDecodeWorker::new_for_opus(
                                        ObservedDecoder {
                                            decoder: lab_decoder::Decoder::new()?,
                                            peak: Arc::clone(&decoded_peak),
                                        },
                                        producer,
                                        0,
                                        OpusFrameConfig::new(48_000, 2, 20)?,
                                    )?;
                                    let address = SocketAddrV4::new(options.bind, 39093).into();
                                    let pipeline = if options.adaptive {
                                        let mut pipeline =
                                            Receiver::bind_with_worker_and_adaptive_target(
                                                address,
                                                grant.root(),
                                                grant.context(),
                                                worker,
                                                Arc::clone(&metrics),
                                                AdaptiveTargetConfig::preset(
                                                    JitterMode::Balanced,
                                                    20,
                                                    8,
                                                    1_000_000_000,
                                                ),
                                            )?;
                                        pipeline.enable_target_gated_playout(
                                            TargetGatedPlayoutConfig { max_gate_ticks: 3 },
                                        )?;
                                        pipeline
                                    } else {
                                        Receiver::bind_with_worker(
                                            address,
                                            grant.root(),
                                            grant.context(),
                                            worker,
                                            Arc::clone(&metrics),
                                        )?
                                    };
                                    Ok((pipeline, next_consumer, metrics))
                                },
                                |(pipeline, _, _)| playback.start(pipeline),
                            )?;
                            let ((pipeline, next_consumer, metrics), started) = match prepared {
                                Ok(result) => result,
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
                            playing = started;
                            receiver = Some(pipeline);
                            consumer = if playing { None } else { Some(next_consumer) };
                            counters = Some(metrics);
                            coordinator.complete_start(token, request)?;
                            next_status = options.status.then(Instant::now);
                            println!("RESULT receiver started");
                            ControlReply::stream_status(
                                request.identity,
                                StreamState::Streaming,
                                "receiver ready",
                            )?
                        }
                    }
                    ControlStreamAction::Stop { token, identity } => {
                        if let (false, Some(pipeline), Some(metrics)) =
                            (draining_stale, &mut receiver, &counters)
                        {
                            drain_stop_tail(
                                pipeline,
                                metrics,
                                &mut consumer,
                                next_playout,
                                &mut decoded,
                                &mut peak,
                            )?;
                        }
                        let output_result = playback.stop();
                        drop(receiver.take());
                        drop(consumer.take());
                        if playing {
                            peak = f32::from_bits(decoded_peak.load(Ordering::Relaxed));
                        }
                        if let Some(metrics) = counters.take() {
                            let snapshot = metrics.snapshot();
                            let now = Instant::now();
                            activity.observe(now, snapshot.authenticated_packets);
                            if options.status {
                                println!(
                                    "{}",
                                    format_status(
                                        snapshot,
                                        decoded,
                                        peak,
                                        "stopped",
                                        options.adaptive
                                    )
                                );
                                println!("MEDIA {}", activity.status(now));
                            }
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
                        println!("RESULT decoded {decoded} peak {peak:.3}");
                        println!(
                            "RESULT schedule decisions {} frame_late {} max_lateness_us {}",
                            schedule.decisions, schedule.frame_late, schedule.max_lateness_us
                        );
                        resyncs.print();
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
                consecutive_plc = 0;
            }
            if draining_stale {
                draining_stale = !drain_stale_media(pipeline)?;
            } else {
                for _ in 0..MAX_MEDIA_POLLS {
                    match poll_media(pipeline)? {
                        None => continue,
                        Some(UdpPollOutcome::Empty) => break,
                        Some(UdpPollOutcome::Processed { admission, .. }) => {
                            if matches!(admission, InsertOutcome::Resynchronized { .. }) {
                                resyncs.observe_admission(
                                    admission,
                                    loop_gap,
                                    previous_decision,
                                    Instant::now(),
                                );
                            }
                            if next_playout.is_none() && retry_at.is_none() {
                                next_playout = Some(Instant::now() + options.startup_buffer);
                            }
                        }
                    }
                }
            }
            let now = Instant::now();
            activity.observe(now, metrics.snapshot().authenticated_packets);
            if let Some(due) = next_playout.filter(|due| now >= *due) {
                schedule.observe(due, now);
                previous_decision = Some((now, now.duration_since(due)));
                match decode_live_playout(pipeline, metrics, &mut consecutive_plc)? {
                    Some(DecodeStep::Decoded { .. }) => decoded += 1,
                    Some(_) => {}
                    None => {
                        next_playout = None;
                        continue;
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
                    options.adaptive,
                ) {
                    println!("{status}");
                    println!("MEDIA {}", activity.status(now));
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

    #[test]
    fn stalled_playout_discards_udp_backlog_and_decodes_fresh_audio() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        assert!(!playout_stalled(true, Some(Duration::from_millis(159))));
        assert!(!playout_stalled(false, Some(Duration::from_secs(1))));
        assert!(playout_stalled(true, Some(Duration::from_millis(160))));
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
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
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let size = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = receiver.local_addr().unwrap();
        for sequence in 0..40 {
            let packet = sealer.seal(0, sequence * 960, &payload[..size]).unwrap();
            sender.send_to(&packet.datagram, address).unwrap();
        }
        receiver.hard_resynchronize();
        assert!(!drain_stale_media(&mut receiver).unwrap());
        assert!(drain_stale_media(&mut receiver).unwrap());
        assert_eq!(receiver.jitter_len(), 0);
        assert_eq!(metrics.snapshot().authenticated_packets, 40);
        let packet = sealer.seal(0, 40 * 960, &payload[..size]).unwrap();
        sender.send_to(&packet.datagram, address).unwrap();
        assert!(matches!(
            poll_media(&mut receiver).unwrap(),
            Some(UdpPollOutcome::Processed { .. })
        ));
        assert!(matches!(
            receiver.decode_next().unwrap(),
            DecodeStep::Decoded { .. }
        ));
        let mut samples = [0.0; SAMPLES];
        consumer.pop_into(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
    }

    #[test]
    fn stop_drain_decodes_buffered_and_late_udp_frames() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let metrics = Arc::new(StreamCounters::default());
        let (producer, consumer) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
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
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let first = sealer.seal(0, 0, &payload[..length]).unwrap();
        let second = sealer.seal(0, 960, &payload[..length]).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = receiver.local_addr().unwrap();
        sender.send_to(&first.datagram, address).unwrap();
        assert!(matches!(
            poll_media(&mut receiver).unwrap(),
            Some(UdpPollOutcome::Processed { .. })
        ));
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            sender.send_to(&second.datagram, address).unwrap();
        });
        let mut decoded = 0;
        let mut peak = 0.0;
        drain_stop_tail(
            &mut receiver,
            &metrics,
            &mut Some(consumer),
            None,
            &mut decoded,
            &mut peak,
        )
        .unwrap();
        thread.join().unwrap();
        assert_eq!(metrics.snapshot().authenticated_packets, 2);
        assert_eq!(metrics.snapshot().jitter_frames, 0);
        assert_eq!(decoded, 2);
        assert!(peak > 0.001);
    }

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
    fn initial_output_failure_reuses_prepared_receiver_on_retry() {
        let identity = MediaStreamIdentity {
            session_id: 1,
            stream_id: 1,
            key_epoch: 0,
        };
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([7; 32]));
        let metrics = Arc::new(StreamCounters::default());
        let factory = sonimesh_app::ReceiverPcmEndpointFactory::new(
            8,
            sonimesh_app::SourcePcmFormat::new(48_000, 2).unwrap(),
            Arc::clone(&metrics),
        );
        let mut output = sonimesh_app::ReceiverOutputController::new(
            FaultOutput {
                reject_start: true,
                ..FaultOutput::default()
            },
            factory,
        );
        let selection = sonimesh_app::OutputSelection {
            device: None,
            config: OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: None,
                maximum_callback_frames: 960,
            },
        };
        let mut pending = None;
        let first: LabResult<_> = start_prepared(
            &mut pending,
            identity,
            || {
                let grant = allocator.supply(identity, MediaDirection::AndroidToDesktop)?;
                let (producer, consumer) = realtime_pcm_queue::<SAMPLES>(8)?;
                let worker = StreamDecodeWorker::new_for_opus(
                    ObservedDecoder {
                        decoder: lab_decoder::Decoder::new()?,
                        peak: Arc::new(AtomicU32::new(0)),
                    },
                    producer,
                    0,
                    OpusFrameConfig::new(48_000, 2, 20)?,
                )?;
                let receiver = Receiver::bind_with_worker(
                    "127.0.0.1:0".parse()?,
                    grant.root(),
                    grant.context(),
                    worker,
                    Arc::clone(&metrics),
                )?;
                Ok((receiver, consumer))
            },
            |(receiver, _)| {
                output.start(selection.clone(), receiver)?;
                Ok(true)
            },
        );
        assert!(first.unwrap().is_err());
        let address = pending.as_ref().unwrap().1.0.local_addr().unwrap();
        assert_eq!(allocator.issued_count(), 1);
        output.output_mut().reject_start = false;
        let second: LabResult<_> = start_prepared(
            &mut pending,
            identity,
            || panic!("retry must retain the original receiver"),
            |(receiver, _)| {
                output.start(selection, receiver)?;
                Ok(true)
            },
        );
        let ((receiver, _), playing) = second.unwrap().unwrap();
        assert!(playing);
        assert_eq!(receiver.local_addr().unwrap(), address);
        assert_eq!(allocator.issued_count(), 1);
        assert_eq!(output.output().starts, 2);
        assert!(pending.is_none());
    }

    #[test]
    fn different_start_retires_failed_preparation() {
        let first = MediaStreamIdentity {
            session_id: 1,
            stream_id: 1,
            key_epoch: 0,
        };
        let second = MediaStreamIdentity {
            session_id: 2,
            ..first
        };
        let mut pending = Some((first, 10));
        let result: Result<_, ()> = start_prepared(&mut pending, second, || Ok(20), |_| Ok(true));
        assert_eq!(result, Ok(Ok((20, true))));
        assert!(pending.is_none());
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
        assert!(decode_playout(&mut receiver, false).unwrap().is_none());
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
            decode_playout(&mut receiver, false).unwrap(),
            Some(DecodeStep::Decoded { sequence: 1, .. })
        ));
        let mut samples = [0.0; SAMPLES];
        consumer.pop_into(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
    }

    #[test]
    fn live_playout_conceals_loss_and_restarts_after_outage() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([0x36; 32]);
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
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let address = receiver.local_addr().unwrap();
        let send = |packet: &[u8]| {
            sender.send_to(packet, address).unwrap();
        };
        let first = sealer.seal(0, 0, &payload[..length]).unwrap();
        send(&first.datagram);
        poll_media(&mut receiver).unwrap();
        let mut consecutive_plc = 0;
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(),
            Some(DecodeStep::Decoded { sequence: 0, .. })
        ));
        let _lost = sealer.seal(0, 960, &payload[..length]).unwrap();
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(),
            Some(DecodeStep::Concealed { sequence: 1, .. })
        ));
        let next = sealer.seal(0, 1920, &payload[..length]).unwrap();
        send(&next.datagram);
        poll_media(&mut receiver).unwrap();
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(),
            Some(DecodeStep::Decoded { sequence: 2, .. })
        ));
        for sequence in 3..6 {
            assert!(
                matches!(decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(), Some(DecodeStep::Concealed { sequence: actual, .. }) if actual == sequence)
            );
        }
        assert!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc)
                .unwrap()
                .is_none()
        );
        let resumed = sealer.seal(0, 2880, &payload[..length]).unwrap();
        send(&resumed.datagram);
        poll_media(&mut receiver).unwrap();
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(),
            Some(DecodeStep::Decoded { sequence: 3, .. })
        ));
        let mut samples = [0.0; SAMPLES];
        consumer.pop_into(&mut samples);
        assert!(samples.iter().any(|sample| sample.abs() > 0.001));
        for index in 4..8 {
            let _ = sealer.seal(0, index * 960, &payload[..length]).unwrap();
        }
        let later = sealer.seal(0, 8 * 960, &payload[..length]).unwrap();
        send(&later.datagram);
        poll_media(&mut receiver).unwrap();
        for sequence in 4..8 {
            assert!(
                matches!(decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(), Some(DecodeStep::Concealed { sequence: actual, .. }) if actual == sequence)
            );
        }
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut consecutive_plc).unwrap(),
            Some(DecodeStep::Decoded { sequence: 8, .. })
        ));
        assert_eq!(metrics.snapshot().plc_frames, 8);
    }

    #[test]
    fn adaptive_live_playout_bounds_waits_recovers_and_drains() {
        use sonimesh_protocol::{MediaKeyContext, MediaSealer};
        use std::net::UdpSocket;

        let secret = SessionSecret::new([0x37; 32]);
        let context = MediaKeyContext::new(8, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
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
        let mut receiver = Receiver::bind_with_worker_and_adaptive_target(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            worker,
            Arc::clone(&metrics),
            AdaptiveTargetConfig::preset(JitterMode::Balanced, 20, 8, 1_000),
        )
        .unwrap();
        receiver
            .enable_target_gated_playout(TargetGatedPlayoutConfig { max_gate_ticks: 3 })
            .unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let length = encoder
            .encode_float(&[0.25; SAMPLES], &mut payload)
            .unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let first = sealer.seal(0, 0, &payload[..length]).unwrap();
        sender
            .send_to(&first.datagram, receiver.local_addr().unwrap())
            .unwrap();
        poll_media(&mut receiver).unwrap();
        let mut plc = 0;
        for _ in 0..3 {
            assert!(matches!(
                decode_live_playout(&mut receiver, &metrics, &mut plc).unwrap(),
                Some(DecodeStep::NotReady)
            ));
        }
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut plc).unwrap(),
            Some(DecodeStep::Decoded { sequence: 0, .. })
        ));
        for sequence in 1..=3 {
            for _ in 0..3 {
                assert!(matches!(
                    decode_live_playout(&mut receiver, &metrics, &mut plc).unwrap(),
                    Some(DecodeStep::NotReady)
                ));
                assert_eq!(plc, sequence - 1);
            }
            assert!(
                matches!(decode_live_playout(&mut receiver, &metrics, &mut plc).unwrap(), Some(DecodeStep::Concealed { sequence: actual, .. }) if actual == u32::from(sequence))
            );
        }
        assert!(
            decode_live_playout(&mut receiver, &metrics, &mut plc)
                .unwrap()
                .is_none()
        );
        let resumed = sealer.seal(0, 960, &payload[..length]).unwrap();
        sender
            .send_to(&resumed.datagram, receiver.local_addr().unwrap())
            .unwrap();
        poll_media(&mut receiver).unwrap();
        assert!(matches!(
            decode_live_playout(&mut receiver, &metrics, &mut plc).unwrap(),
            Some(DecodeStep::NotReady)
        ));
        assert!(matches!(
            decode_playout(&mut receiver, false).unwrap(),
            Some(DecodeStep::Decoded { sequence: 1, .. })
        ));
    }

    #[test]
    fn schedule_observations_record_late_decisions_and_maximum() {
        let due = Instant::now();
        let mut observations = ScheduleObservations::default();
        observations.observe(due, due + Duration::from_millis(19));
        observations.observe(due, due + Duration::from_millis(20));
        observations.observe(due, due + Duration::from_micros(41_500));
        assert_eq!(observations.decisions, 3);
        assert_eq!(observations.frame_late, 2);
        assert_eq!(observations.max_lateness_us, 41_500);
    }

    #[test]
    fn resync_observations_retain_first_events_and_count_the_rest() {
        let mut observations = ResyncObservations::default();
        for dropped in 0..12 {
            observations.record(ResyncObservation {
                dropped,
                loop_gap: Some(Duration::from_millis(dropped as u64)),
                previous_decision: (dropped != 0)
                    .then_some((Duration::from_millis(5), Duration::from_millis(2))),
            });
        }
        assert_eq!(observations.total, 12);
        let first = observations.first[0].unwrap();
        assert_eq!(first.dropped, 0);
        assert_eq!(first.previous_decision, None);
        let last = observations.first[7].unwrap();
        assert_eq!(last.dropped, 7);
        assert_eq!(last.loop_gap, Some(Duration::from_millis(7)));
        assert_eq!(
            last.previous_decision,
            Some((Duration::from_millis(5), Duration::from_millis(2)))
        );
    }

    #[test]
    fn resync_observations_capture_real_udp_buffer_overflow() {
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
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let mut observations = ResyncObservations::default();
        let now = Instant::now();
        for sequence in 0..9 {
            let packet = sealer.seal(0, sequence * 960, &[1]).unwrap();
            sender
                .send_to(&packet.datagram, receiver.local_addr().unwrap())
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(UdpPollOutcome::Processed { admission, .. }) =
                    poll_media(&mut receiver).unwrap()
                {
                    observations.observe_admission(
                        admission,
                        Some(Duration::from_millis(3)),
                        Some((now, Duration::from_millis(1))),
                        now + Duration::from_millis(5),
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "UDP packet was not received");
                std::thread::yield_now();
            }
        }
        assert_eq!(metrics.snapshot().jitter_resync_dropped_frames, 8);
        assert_eq!(observations.total, 1);
        let event = observations.first[0].unwrap();
        assert_eq!(event.dropped, 8);
        assert_eq!(event.loop_gap, Some(Duration::from_millis(3)));
        assert_eq!(
            event.previous_decision,
            Some((Duration::from_millis(5), Duration::from_millis(1)))
        );
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
    fn playout_deadline_skips_missed_ticks_and_limits_catchup() {
        let deadline = Instant::now();
        let almost_next = deadline + Duration::from_millis(19);
        assert_eq!(
            next_playout_deadline(deadline, almost_next),
            almost_next + Duration::from_millis(10)
        );
        let late = deadline + Duration::from_millis(21);
        assert_eq!(
            next_playout_deadline(deadline, late),
            deadline + Duration::from_millis(40)
        );
        let much_later = deadline + Duration::from_secs(5);
        assert_eq!(
            next_playout_deadline(deadline, much_later),
            much_later + PLAYOUT_PERIOD
        );
    }

    #[test]
    fn playout_deadline_maintains_bounded_spacing_under_variable_delays() {
        let mut deadline = Instant::now();
        for delay_ms in [0, 2, 9, 10, 19, 20, 21, 29, 39, 41, 500]
            .into_iter()
            .cycle()
            .take(1_000)
        {
            let now = deadline + Duration::from_millis(delay_ms);
            let next = next_playout_deadline(deadline, now);
            assert!(next.duration_since(now) >= Duration::from_millis(10));
            assert!(next.duration_since(now) <= PLAYOUT_PERIOD);
            deadline = next;
        }
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
        assert!(!defaults.adaptive);
        assert_eq!(defaults.startup_buffer, Duration::from_millis(60));
        for value in ["20", "40", "60", "80", "100", "120"] {
            let options = Options::parse(args(&["--startup-buffer-ms", value])).unwrap();
            assert_eq!(
                options.startup_buffer.as_millis(),
                value.parse::<u128>().unwrap()
            );
        }
        for value in ["0", "19", "30", "140", "invalid"] {
            assert!(Options::parse(args(&["--startup-buffer-ms", value])).is_err());
        }
        assert!(Options::parse(args(&["--startup-buffer-ms"])).is_err());
        assert!(
            Options::parse(args(&[
                "--startup-buffer-ms",
                "20",
                "--startup-buffer-ms",
                "40"
            ]))
            .is_err()
        );
        assert!(Options::parse(args(&["--status"])).unwrap().status);
        assert!(Options::parse(args(&["--status", "--status"])).is_err());
        assert!(Options::parse(args(&["--adaptive"])).unwrap().adaptive);
        assert!(Options::parse(args(&["--adaptive", "--adaptive"])).is_err());
        let playback =
            Options::parse(args(&["--require-tone", "--play", "--select-output"])).unwrap();
        assert!(playback.play && playback.require_tone && playback.select_output);
        assert!(Options::parse(args(&["--select-output"])).is_err());
        assert!(Options::parse(args(&["--output-id-stdin"])).is_err());
        assert!(Options::parse(args(&["--output-id-framed"])).is_err());
        assert!(Options::parse(args(&["--play", "--output-id-framed", "--control"])).is_ok());
        assert!(Options::parse(args(&["--play", "--output-id-stdin", "--control"])).is_err());
        assert!(
            Options::parse(args(&["--play", "--output-id-framed", "--output-id-stdin"])).is_err()
        );
        assert!(
            Options::parse(args(&["--play", "--output-id-stdin"]))
                .unwrap()
                .output_id_stdin
        );
        assert!(Options::parse(args(&["--play", "--output-id-stdin", "--select-output"])).is_err());
        assert!(
            Options::parse(args(&["--play", "--output-id-stdin", "--output-id-stdin"])).is_err()
        );
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
    fn rendered_lab_invitation_is_bounded_and_decodes_to_the_listener_host() {
        let payload = render_lab_invitation(&[0xAB; 32], Ipv4Addr::new(10, 0, 2, 2)).unwrap();
        assert_eq!(payload.len(), 67 + "10.0.2.2".len() + 32);
        let decoded = sonimesh_protocol::parse_pairing_qr_payload(&payload).unwrap();
        assert_eq!(decoded.pairing_id, LAB_ID);
        assert_eq!(decoded.pairing_secret, LAB_SECRET);
        assert_eq!(decoded.control_host, "10.0.2.2");
        assert_eq!(decoded.control_port, 47610);
        assert_eq!(decoded.desktop_tls_spki, &[0xAB; 32]);
        assert_eq!(bytes_to_hex(&payload).len(), payload.len() * 2);
    }

    #[test]
    fn live_status_reports_media_progress_and_output_recovery() {
        let now = Instant::now();
        let mut next = Some(now + STATUS_PERIOD);
        let metrics = StreamMetricsSnapshot {
            authenticated_packets: 120,
            rejected_packets: 3,
            late_packets: 9,
            jitter_resync_dropped_frames: 6,
            plc_frames: 2,
            decode_errors: 1,
            jitter_frames: 4,
            target_frames: 3,
            target_min_frames: 2,
            target_max_frames: 4,
            target_smoothed_jitter_ms: 17,
            target_gated_ticks: 7,
            target_forced_playouts: 2,
            stream_resets: 1,
            ..StreamMetricsSnapshot::default()
        };
        assert!(status_if_due(&mut next, now, metrics, 115, 0.25, "recovering", true).is_none());
        let late = now + STATUS_PERIOD * 3;
        assert_eq!(
            status_if_due(&mut next, late, metrics, 115, 0.25, "recovering", true).as_deref(),
            Some(
                "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output recovering late 9 resync_dropped 6 playout adaptive target 3 min 2 max 4 jitter_ms 17 gated 7 forced 2 resets 1"
            )
        );
        assert!(status_if_due(&mut next, late, metrics, 115, 0.25, "recovering", true).is_none());
        assert_eq!(
            status_if_due(
                &mut next,
                late + STATUS_PERIOD,
                StreamMetricsSnapshot::default(),
                0,
                0.0,
                "software-check",
                false
            )
            .as_deref(),
            Some(
                "STATUS packets 0 rejected 0 decoded 0 peak 0.000 plc 0 decode_errors 0 buffered 0 output software-check late 0 resync_dropped 0 playout fixed"
            )
        );
        assert!(status_if_due(&mut None, late, metrics, 0, 0.0, "active", false).is_none());
    }

    #[test]
    fn media_activity_distinguishes_first_packet_gap_and_resumption() {
        let now = Instant::now();
        let mut activity = MediaActivity::default();
        activity.observe(now, 0);
        assert_eq!(activity.status(now + Duration::from_secs(20)), "waiting");
        activity.observe(now + Duration::from_secs(20), 1);
        assert_eq!(
            activity.status(now + Duration::from_millis(21_999)),
            "receiving"
        );
        activity.observe(now + Duration::from_secs(22), 1);
        assert_eq!(
            activity.status(now + Duration::from_secs(22)),
            "interrupted"
        );
        activity.observe(now + Duration::from_secs(23), 2);
        assert_eq!(activity.status(now + Duration::from_secs(23)), "receiving");
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
