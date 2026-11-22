//! Controlled-network UDP lab. ADB bootstrap is not authenticated pairing.
mod common;

use std::{
    error::Error,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverPcmEndpointFactory, ReceiverPipelineError,
    SourcePcmFormat, UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, DecodeWorkerError, FrameDecoder, GainControlHandle, InsertOutcome, OpusFrameConfig,
    StreamDecodeWorker, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{IngressError, UdpReceiveError};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

const SAMPLES: usize = 1_920;
type LabResult<T> = Result<T, Box<dyn Error>>;
type LabReceiver = UdpReceiverPipeline<Decoder, 8, 1_148, SAMPLES>;

struct Decoder(opus::Decoder);
impl FrameDecoder for Decoder {
    type Error = opus::Error;
    fn decode(&mut self, input: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        self.0.decode_float(input, output, false).map(|n| n * 2)
    }
    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        self.0.decode_float(&[], output, false).map(|n| n * 2)
    }
}

trait LabOutput: AudioOutput {
    fn tick(&mut self) -> LabResult<()>;
    fn finish(&mut self) -> LabResult<()>;
}

#[derive(Default)]
struct CheckOutput {
    source: Option<Box<dyn RenderSource>>,
    peak: f32,
}
impl AudioOutput for CheckOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        Ok(Vec::new())
    }
    fn start(
        &mut self,
        _: Option<&OutputDeviceId>,
        _: OutputConfig,
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
impl LabOutput for CheckOutput {
    fn tick(&mut self) -> LabResult<()> {
        let mut samples = [0_f32; SAMPLES];
        self.source
            .as_mut()
            .ok_or("output not started")?
            .render(&mut samples);
        for value in samples {
            if !value.is_finite() {
                return Err("non-finite render sample".into());
            }
            self.peak = self.peak.max(value.abs());
        }
        Ok(())
    }
    fn finish(&mut self) -> LabResult<()> {
        println!("software render peak: {:.3}", self.peak);
        if self.peak < 0.001 {
            println!(
                "No audible output detected; the source may be silent or capture-limited, or the output may be muted or attenuated."
            );
        }
        Ok(())
    }
}

#[cfg(all(windows, feature = "playback"))]
impl LabOutput for sonimesh_platform::CpalOutput {
    fn tick(&mut self) -> LabResult<()> {
        Ok(())
    }
    fn finish(&mut self) -> LabResult<()> {
        std::thread::sleep(Duration::from_millis(160));
        let status = self.status();
        println!(
            "output callbacks: {}, stream errors: {}",
            status.callback_count, status.stream_errors
        );
        if status.callback_count == 0
            || status.stream_errors != 0
            || status.oversized_callbacks != 0
        {
            return Err("output did not render cleanly".into());
        }
        Ok(())
    }
}

fn read_secret(control: &mut TcpStream) -> LabResult<SessionSecret> {
    control.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = [0_u8; 32];
    control.read_exact(&mut bytes)?;
    let secret = SessionSecret::new(bytes);
    bytes.fill(0);
    Ok(secret)
}

fn control_closed(control: &mut TcpStream) -> LabResult<bool> {
    let mut byte = [0_u8; 1];
    match control.read(&mut byte) {
        Ok(0) => Ok(true),
        Ok(_) => Err("unexpected lab control data".into()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn decode_buffered(
    receiver: &mut LabReceiver,
    counters: &StreamCounters,
) -> Result<DecodeStep, DecodeWorkerError<opus::Error>> {
    if counters.snapshot().jitter_frames == 0 {
        Ok(DecodeStep::NotReady)
    } else {
        receiver.decode_next()
    }
}

fn poll_media(receiver: &mut LabReceiver) -> LabResult<Option<UdpPollOutcome>> {
    match receiver.poll() {
        Ok(outcome) => Ok(Some(outcome)),
        Err(
            UdpReceiverError::Socket(UdpReceiveError::DatagramTooLarge)
            | UdpReceiverError::SourceEndpointChanged
            | UdpReceiverError::Pipeline(ReceiverPipelineError::Ingress(
                IngressError::Parse(_)
                | IngressError::UnknownStream
                | IngressError::Authentication(_)
                | IngressError::Replay(_),
            )),
        ) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn apply_control(input: &str, gain: &GainControlHandle) -> Result<(), &'static str> {
    let mut words = input.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("mute"), None, None) => gain.set_muted(true),
        (Some("unmute"), None, None) => gain.set_muted(false),
        (Some("volume"), Some(value), None) => {
            let percent: u8 = value.parse().map_err(|_| "volume must be 0 to 100")?;
            if percent > 100 {
                return Err("volume must be 0 to 100");
            }
            gain.set_gain(f32::from(percent) / 100.0)
                .map_err(|_| "invalid volume")?;
        }
        _ => return Err("use: volume 0-100 | mute | unmute"),
    }
    Ok(())
}

fn start_console_control(enabled: bool) -> GainControlHandle {
    let gain = GainControlHandle::new();
    if enabled {
        let control = gain.clone();
        std::thread::spawn(move || {
            println!("Audio controls: volume 0-100 | mute | unmute");
            let mut input = io::stdin().lock();
            let mut buffer = [0_u8; 64];
            let mut length = 0;
            let mut overflow = false;
            loop {
                let mut byte = [0_u8; 1];
                match input.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) if byte[0] == b'\n' => {
                        let result = if overflow {
                            Err("command too long")
                        } else {
                            std::str::from_utf8(&buffer[..length])
                                .map_err(|_| "invalid command")
                                .and_then(|line| apply_control(line, &control))
                        };
                        match result {
                            Ok(()) => println!("Audio control updated."),
                            Err(error) => eprintln!("{error}"),
                        }
                        length = 0;
                        overflow = false;
                    }
                    Ok(_) if length < buffer.len() && !overflow => {
                        buffer[length] = byte[0];
                        length += 1;
                    }
                    Ok(_) => overflow = true,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        eprintln!("Audio controls unavailable: {error}");
                        break;
                    }
                }
            }
        });
    }
    gain
}

fn sync_gain(source: &GainControlHandle, target: &GainControlHandle) -> LabResult<()> {
    let state = source.snapshot();
    target.set_gain(state.gain)?;
    target.set_muted(state.muted);
    Ok(())
}

#[allow(clippy::too_many_lines)] // One lab session coordinates socket, playout, and output lifetimes.
fn run<O: LabOutput>(
    mut control: TcpStream,
    output: O,
    selection: OutputSelection,
    udp_bind: Ipv4Addr,
    gain: &GainControlHandle,
) -> LabResult<()> {
    let secret = read_secret(&mut control)?;
    let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop)?;
    let counters = Arc::new(StreamCounters::default());
    let (producer, _unused) = realtime_pcm_queue::<SAMPLES>(8)?;
    let worker = StreamDecodeWorker::<_, 8, 1_148, SAMPLES>::new_for_opus(
        Decoder(opus::Decoder::new(48_000, opus::Channels::Stereo)?),
        producer,
        0,
        OpusFrameConfig::new(48_000, 2, 20)?,
    )?;
    let mut receiver = UdpReceiverPipeline::bind_with_worker(
        SocketAddr::V4(SocketAddrV4::new(udp_bind, 39093)),
        &secret,
        context,
        worker,
        Arc::clone(&counters),
    )?;
    let factory =
        ReceiverPcmEndpointFactory::new(8, SourcePcmFormat::new(48_000, 2)?, Arc::clone(&counters));
    let mut output = ReceiverOutputController::new(output, factory);
    let output_gain = output.gain_handle();
    sync_gain(gain, &output_gain)?;
    output.start(selection, &mut receiver)?;
    control.write_all(&[1])?;
    control.set_nonblocking(true)?;
    let mut last_packet = Instant::now();
    let mut next_playout: Option<Instant> = None;
    let mut admitted = 0_u64;
    let mut late = 0_u64;
    let mut resynchronized = 0_u64;
    let mut decoded = 0_u64;
    let mut concealed = 0_u64;
    let mut empty_ticks = 0_u64;
    loop {
        sync_gain(gain, &output_gain)?;
        if control_closed(&mut control)? {
            break;
        }
        for _ in 0..32 {
            match poll_media(&mut receiver)? {
                None => continue,
                Some(UdpPollOutcome::Empty) => break,
                Some(UdpPollOutcome::Processed { admission, .. }) => {
                    match admission {
                        InsertOutcome::Inserted | InsertOutcome::Reordered => admitted += 1,
                        InsertOutcome::Resynchronized { .. } => {
                            admitted += 1;
                            resynchronized += 1;
                        }
                        InsertOutcome::Late => late += 1,
                        _ => return Err("unexpected media admission outcome".into()),
                    }
                    last_packet = Instant::now();
                    if next_playout.is_none() {
                        next_playout = Some(last_packet + Duration::from_millis(60));
                    }
                }
            }
        }
        let now = Instant::now();
        if let Some(deadline) = next_playout {
            if now >= deadline {
                match decode_buffered(&mut receiver, &counters)? {
                    DecodeStep::Decoded {
                        samples: SAMPLES, ..
                    } => decoded += 1,
                    DecodeStep::Concealed {
                        samples: SAMPLES, ..
                    } => concealed += 1,
                    DecodeStep::NotReady => empty_ticks += 1,
                    _ => return Err("decoder produced an unexpected frame shape".into()),
                }
                output.output_mut().tick()?;
                if output.monitor().is_some() {
                    return Err("output device reported a fault".into());
                }
                next_playout = Some(
                    if now.duration_since(deadline) >= Duration::from_millis(20) {
                        now + Duration::from_millis(20)
                    } else {
                        deadline + Duration::from_millis(20)
                    },
                );
            }
        }
        if last_packet.elapsed() > Duration::from_secs(5) {
            return Err("media receiver timed out".into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    println!(
        "media packets admitted: {admitted}, late: {late}, resynchronized: {resynchronized}, decoded: {decoded}, concealed: {concealed}, empty ticks: {empty_ticks}"
    );
    output.output_mut().finish()?;
    output.stop();
    if decoded == 0 {
        return Err("no media frames decoded".into());
    }
    Ok(())
}

fn main() -> LabResult<()> {
    let (mode, udp_bind, repeat, control) = parse_args(std::env::args().skip(1))?;
    match mode {
        common::Mode::Check => {
            let config = OutputConfig {
                sample_rate_hz: 48_000,
                channels: 2,
                sample_format: SampleFormat::Float32,
                requested_callback_frames: None,
                maximum_callback_frames: 960,
            };
            let selection = OutputSelection {
                device: None,
                config,
            };
            let gain = start_console_control(control);
            serve_control(repeat, |control| {
                run(
                    control,
                    CheckOutput::default(),
                    selection.clone(),
                    udp_bind,
                    &gain,
                )
            })
        }
        common::Mode::Play { select_output } => play(select_output, udp_bind, repeat, control),
    }
}

fn parse_args(
    args: impl IntoIterator<Item = String>,
) -> LabResult<(common::Mode, Ipv4Addr, bool, bool)> {
    let mut args = args.into_iter();
    let mut common_args = Vec::new();
    let mut bind = None;
    let mut repeat = false;
    let mut control = false;
    while let Some(arg) = args.next() {
        if arg == "--bind" {
            if bind.is_some() {
                return Err("--bind may only be supplied once".into());
            }
            let address: Ipv4Addr = args
                .next()
                .ok_or("--bind requires an IPv4 address")?
                .parse()?;
            if address.is_multicast() || address.is_broadcast() {
                return Err("--bind must be a local unicast or unspecified address".into());
            }
            bind = Some(address);
        } else if arg == "--repeat" {
            if repeat {
                return Err("--repeat may only be supplied once".into());
            }
            repeat = true;
        } else if arg == "--control" {
            if control {
                return Err("--control may only be supplied once".into());
            }
            control = true;
        } else {
            common_args.push(arg);
        }
    }
    Ok((
        common::parse_args(common_args)?,
        bind.unwrap_or(Ipv4Addr::LOCALHOST),
        repeat,
        control,
    ))
}

fn serve_control(repeat: bool, handler: impl FnMut(TcpStream) -> LabResult<()>) -> LabResult<()> {
    let listener = TcpListener::bind("127.0.0.1:39092")?;
    println!("Listening for ADB lab bootstrap on 127.0.0.1:39092; UDP uses port 39093.");
    serve_sessions(&listener, if repeat { None } else { Some(1) }, handler)
}

fn serve_sessions(
    listener: &TcpListener,
    limit: Option<usize>,
    mut handler: impl FnMut(TcpStream) -> LabResult<()>,
) -> LabResult<()> {
    let mut count = 0;
    loop {
        let (control, _) = listener.accept()?;
        let result = handler(control);
        count += 1;
        if limit == Some(count) {
            return result;
        }
        if let Err(error) = result {
            eprintln!("Lab session ended: {error}");
        }
        println!("Waiting for the next lab session.");
    }
}

#[cfg(all(windows, feature = "playback"))]
fn play(select_output: bool, udp_bind: Ipv4Addr, repeat: bool, control: bool) -> LabResult<()> {
    let (_, selection) = common::prepare_output(select_output)?;
    let gain = start_console_control(control);
    serve_control(repeat, |control| {
        run(
            control,
            sonimesh_platform::CpalOutput::new(),
            selection.clone(),
            udp_bind,
            &gain,
        )
    })
}
#[cfg(not(all(windows, feature = "playback")))]
fn play(_: bool, _: Ipv4Addr, _: bool, _: bool) -> LabResult<()> {
    Err("playback requires Windows and --features playback".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonimesh_protocol::MediaSealer;
    use std::net::UdpSocket;

    #[test]
    fn bind_address_is_opt_in_and_validated() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parse_args(args(&["--check"])).unwrap().1,
            Ipv4Addr::LOCALHOST
        );
        assert_eq!(
            parse_args(args(&["--play", "--bind", "0.0.0.0", "--select-output"]))
                .unwrap()
                .1,
            Ipv4Addr::UNSPECIFIED
        );
        assert!(parse_args(args(&["--check", "--bind", "example.com"])).is_err());
        assert!(parse_args(args(&["--check", "--bind", "224.0.0.1"])).is_err());
        assert!(parse_args(args(&["--check", "--repeat"])).unwrap().2);
        assert!(!parse_args(args(&["--check"])).unwrap().2);
        assert!(parse_args(args(&["--check", "--repeat", "--repeat"])).is_err());
        assert!(parse_args(args(&["--check", "--control"])).unwrap().3);
        assert!(parse_args(args(&["--check", "--control", "--control"])).is_err());
        assert!(
            parse_args(args(&[
                "--check",
                "--bind",
                "0.0.0.0",
                "--bind",
                "127.0.0.1"
            ]))
            .is_err()
        );
    }

    #[test]
    fn console_controls_change_rendered_audio_and_carry_to_a_fresh_session() {
        let control = GainControlHandle::new();
        assert!(apply_control("volume 101", &control).is_err());
        assert!(apply_control("volume NaN", &control).is_err());
        assert!(apply_control("mute extra", &control).is_err());
        apply_control("volume 50", &control).unwrap();
        let config = OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: None,
            maximum_callback_frames: 960,
        };
        for session in 0..2 {
            let factory = ReceiverPcmEndpointFactory::<SAMPLES>::new(
                8,
                SourcePcmFormat::new(48_000, 2).unwrap(),
                Arc::new(StreamCounters::default()),
            );
            let endpoint = factory.create(&config).unwrap();
            let output_gain = endpoint.gain_handle();
            sync_gain(&control, &output_gain).unwrap();
            let (mut producer, mut source) = endpoint.into_parts();
            producer.push(&[0.5; SAMPLES]);
            let mut samples = [0.0; SAMPLES];
            source.render(&mut samples);
            assert!((samples[SAMPLES - 1] - 0.25).abs() < 0.001);
            if session == 0 {
                apply_control("mute", &control).unwrap();
                sync_gain(&control, &output_gain).unwrap();
                producer.push(&[0.5; SAMPLES]);
                source.render(&mut samples);
                assert!(samples[SAMPLES - 1].abs() < 0.001);
                apply_control("unmute", &control).unwrap();
            }
        }
    }

    #[test]
    fn repeated_listener_accepts_another_session_after_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            for _ in 0..2 {
                let mut stream = TcpStream::connect(address).unwrap();
                stream.write_all(&[1]).unwrap();
            }
        });
        let mut sessions = 0;
        serve_sessions(&listener, Some(2), |mut stream| {
            let mut data = [0];
            stream.read_exact(&mut data)?;
            assert_eq!(data, [1]);
            sessions += 1;
            if sessions == 1 {
                Err("first session failed".into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        client.join().unwrap();
        assert_eq!(sessions, 2);
    }

    #[test]
    fn empty_jitter_waits_without_losing_future_frames_and_real_gap_uses_plc() {
        let secret = SessionSecret::new([7; 32]);
        let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let counters = Arc::new(StreamCounters::default());
        let (producer, _consumer) = realtime_pcm_queue::<SAMPLES>(8).unwrap();
        let worker = StreamDecodeWorker::new_for_opus(
            Decoder(opus::Decoder::new(48_000, opus::Channels::Stereo).unwrap()),
            producer,
            0,
            OpusFrameConfig::new(48_000, 2, 20).unwrap(),
        )
        .unwrap();
        let mut receiver = LabReceiver::bind_with_worker(
            "127.0.0.1:0".parse().unwrap(),
            &secret,
            context,
            worker,
            Arc::clone(&counters),
        )
        .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut sealer = MediaSealer::new(&secret, context).unwrap();
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut payload = [0_u8; 1_148];
        let len = encoder
            .encode_float(&[0.05; SAMPLES], &mut payload)
            .unwrap();
        socket
            .send_to(b"invalid", receiver.local_addr().unwrap())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match poll_media(&mut receiver).unwrap() {
                None => break,
                Some(UdpPollOutcome::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                _ => panic!("invalid datagram was not rejected"),
            }
        }
        let first = sealer.seal(0, 0, &payload[..len]).unwrap();
        socket
            .send_to(&first.datagram, receiver.local_addr().unwrap())
            .unwrap();
        poll_packet(&mut receiver);
        assert!(matches!(
            decode_buffered(&mut receiver, &counters).unwrap(),
            DecodeStep::Decoded { sequence: 0, .. }
        ));
        for _ in 0..20 {
            assert!(matches!(
                decode_buffered(&mut receiver, &counters).unwrap(),
                DecodeStep::NotReady
            ));
        }
        let _dropped = sealer.seal(0, 960, &payload[..len]).unwrap();
        let third = sealer.seal(0, 1_920, &payload[..len]).unwrap();
        socket
            .send_to(&third.datagram, receiver.local_addr().unwrap())
            .unwrap();
        poll_packet(&mut receiver);
        assert!(matches!(
            decode_buffered(&mut receiver, &counters).unwrap(),
            DecodeStep::Concealed { sequence: 1, .. }
        ));
        assert!(matches!(
            decode_buffered(&mut receiver, &counters).unwrap(),
            DecodeStep::Decoded { sequence: 2, .. }
        ));
    }

    fn poll_packet(receiver: &mut LabReceiver) {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match poll_media(receiver).unwrap() {
                Some(UdpPollOutcome::Processed { .. }) => return,
                None | Some(UdpPollOutcome::Empty) if Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                None | Some(UdpPollOutcome::Empty) => panic!("loopback packet timed out"),
            }
        }
    }
}
