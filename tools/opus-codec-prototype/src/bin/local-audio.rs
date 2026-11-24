//! Loopback-only Opus-to-output lab. The fixed test key is not a pairing key.
mod lab_decoder;

use lab_decoder::Decoder;

use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverPcmEndpointFactory, SourcePcmFormat,
    UdpPollOutcome, UdpReceiverPipeline,
};
use sonimesh_audio::{DecodeStep, OpusFrameConfig, StreamDecodeWorker, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{MediaSessionSendOutcome, MediaSessionSender};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

const SAMPLES: usize = 1_920;
const FRAMES: u32 = 100;
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const STARTUP_BUFFER: Duration = Duration::from_millis(60);
type LabResult<T> = Result<T, Box<dyn Error>>;

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

impl LabOutput for CheckOutput {
    fn tick(&mut self) -> LabResult<()> {
        let mut samples = [0.0_f32; SAMPLES];
        self.source
            .as_mut()
            .ok_or("output not started")?
            .render(&mut samples);
        for sample in samples {
            if !sample.is_finite() {
                return Err("rendered sample was not finite".into());
            }
            self.peak = self.peak.max(sample.abs());
        }
        Ok(())
    }

    fn finish(&mut self) -> LabResult<()> {
        if self.peak < 0.001 {
            return Err("rendered output was silent".into());
        }
        Ok(())
    }
}

#[cfg(all(windows, feature = "playback"))]
impl LabOutput for sonimesh_platform::CpalOutput {
    fn tick(&mut self) -> LabResult<()> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    }

    fn finish(&mut self) -> LabResult<()> {
        // Allow queued audio to reach the device before closing the stream.
        std::thread::sleep(Duration::from_millis(160));
        let status = self.status();
        if status.callback_count == 0 || status.stream_errors > 0 || status.oversized_callbacks > 0
        {
            return Err("output did not render cleanly".into());
        }
        println!("output callbacks: {}", status.callback_count);
        Ok(())
    }
}

fn run<O: LabOutput>(output: O, config: OutputConfig, relay: Option<(u16, u16)>) -> LabResult<()> {
    let frame = OpusFrameConfig::new(48_000, 2, 20)?;
    let secret = SessionSecret::new([0x42; 32]);
    let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop)?;
    let counters = Arc::new(StreamCounters::default());
    let (producer, _unused_consumer) = realtime_pcm_queue::<SAMPLES>(8)?;
    let decoder = Decoder::new()?;
    let worker =
        StreamDecodeWorker::<_, 8, 1_148, SAMPLES>::new_for_opus(decoder, producer, 0, frame)?;
    let mut receiver = UdpReceiverPipeline::bind_with_worker(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, relay.map_or(0, |(_, receive)| receive)).into(),
        &secret,
        context,
        worker,
        Arc::clone(&counters),
    )?;
    let destination: SocketAddr = relay.map_or_else(
        || receiver.local_addr(),
        |(listen, _)| Ok(SocketAddrV4::new(Ipv4Addr::LOCALHOST, listen).into()),
    )?;
    let mut sender =
        MediaSessionSender::bind("127.0.0.1:0".parse()?, &secret, context, destination)?;
    let factory = ReceiverPcmEndpointFactory::new(8, SourcePcmFormat::new(48_000, 2)?, counters);
    let mut output = ReceiverOutputController::new(output, factory);
    output.start(
        OutputSelection {
            device: None,
            config,
        },
        &mut receiver,
    )?;
    let mut encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
    let mut pcm = [0.0_f32; SAMPLES];
    let mut packet = [0_u8; 1_148];
    for sequence in 0..FRAMES {
        for (index, stereo) in pcm.chunks_exact_mut(2).enumerate() {
            let phase = u16::try_from(index % 48)?;
            let value = (f32::from(phase) * std::f32::consts::TAU / 48.0).sin() * 0.08;
            stereo.fill(value);
        }
        let len = encoder.encode_float(&pcm, &mut packet)?;
        if !matches!(sender.send_frame(0, u64::from(sequence) * 960, &packet[..len])?,
            MediaSessionSendOutcome::Sent { sequence: sent } if sent == sequence)
        {
            return Err("loopback sender did not send the expected frame".into());
        }
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            match receiver.poll()? {
                UdpPollOutcome::Processed { .. } => break,
                UdpPollOutcome::Empty if Instant::now() < until => std::thread::yield_now(),
                UdpPollOutcome::Empty => return Err("loopback receiver timed out".into()),
            }
        }
        if !matches!(receiver.decode_next()?, DecodeStep::Decoded { sequence: actual, samples: SAMPLES, .. } if actual == sequence)
        {
            return Err("receiver did not decode the expected frame".into());
        }
        output.output_mut().tick()?;
        if output.monitor().is_some() {
            return Err("output device reported a fault".into());
        }
    }
    output.output_mut().finish()?;
    output.stop();
    Ok(())
}

fn check(relay: Option<(u16, u16)>) -> LabResult<()> {
    let config = OutputConfig {
        sample_rate_hz: 48_000,
        channels: 2,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: None,
        maximum_callback_frames: 960,
    };
    run(CheckOutput::default(), config, relay)?;
    println!("loopback audio check passed: {FRAMES} Opus frames, non-silent render");
    Ok(())
}

fn send_paced_frame(
    sender: &mut MediaSessionSender,
    encoder: &mut opus::Encoder,
    pcm: &mut [f32; SAMPLES],
    packet: &mut [u8; 1_148],
    sequence: u32,
) -> LabResult<()> {
    for (index, stereo) in pcm.chunks_exact_mut(2).enumerate() {
        let phase = u16::try_from(index % 48)?;
        let value = (f32::from(phase) * std::f32::consts::TAU / 48.0).sin() * 0.08;
        stereo.fill(value);
    }
    let len = encoder.encode_float(pcm, packet)?;
    if !matches!(sender.send_frame(0, u64::from(sequence) * 960, &packet[..len])?,
        MediaSessionSendOutcome::Sent { sequence: sent } if sent == sequence)
    {
        return Err("paced sender did not send the expected frame".into());
    }
    Ok(())
}

fn stream_via_relay(relay: (u16, u16)) -> LabResult<()> {
    let frame = OpusFrameConfig::new(48_000, 2, 20)?;
    let secret = SessionSecret::new([0x42; 32]);
    let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop)?;
    let counters = Arc::new(StreamCounters::default());
    let (producer, _unused_consumer) = realtime_pcm_queue::<SAMPLES>(8)?;
    let decoder = Decoder::new()?;
    let worker =
        StreamDecodeWorker::<_, 8, 1_148, SAMPLES>::new_for_opus(decoder, producer, 0, frame)?;
    let mut receiver = UdpReceiverPipeline::bind_with_worker(
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, relay.1).into(),
        &secret,
        context,
        worker,
        Arc::clone(&counters),
    )?;
    let destination: SocketAddr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, relay.0).into();
    let mut sender =
        MediaSessionSender::bind("127.0.0.1:0".parse()?, &secret, context, destination)?;
    let factory = ReceiverPcmEndpointFactory::new(8, SourcePcmFormat::new(48_000, 2)?, counters);
    let mut output = ReceiverOutputController::new(CheckOutput::default(), factory);
    output.start(
        OutputSelection {
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
    )?;
    let mut encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)?;
    let mut pcm = [0.0_f32; SAMPLES];
    let mut packet = [0_u8; 1_148];
    let start = Instant::now();
    let mut next_send = start;
    let mut next_playout = start + STARTUP_BUFFER;
    let mut sent = 0_u32;
    let mut playout_ticks = 0_u32;
    let mut admitted = 0_u64;
    let mut decoded_frames = 0_u64;
    let mut concealed = 0_u64;
    let mut empty_ticks = 0_u64;

    while sent < FRAMES || playout_ticks < FRAMES {
        let now = Instant::now();
        if sent < FRAMES && now >= next_send {
            send_paced_frame(&mut sender, &mut encoder, &mut pcm, &mut packet, sent)?;
            sent += 1;
            next_send += FRAME_INTERVAL;
            if next_send <= Instant::now() {
                next_send = Instant::now() + FRAME_INTERVAL;
            }
        }

        for _ in 0..32 {
            match receiver.poll()? {
                UdpPollOutcome::Processed { .. } => admitted += 1,
                UdpPollOutcome::Empty => break,
            }
        }

        let now = Instant::now();
        if now >= next_playout && playout_ticks < FRAMES {
            match receiver.decode_next()? {
                DecodeStep::Decoded {
                    samples: SAMPLES, ..
                } => decoded_frames += 1,
                DecodeStep::Concealed {
                    samples: SAMPLES, ..
                } => concealed += 1,
                DecodeStep::NotReady => empty_ticks += 1,
                _ => return Err("paced decoder produced an unexpected frame shape".into()),
            }
            output.output_mut().tick()?;
            if output.monitor().is_some() {
                return Err("output device reported a fault".into());
            }
            playout_ticks += 1;
            next_playout += FRAME_INTERVAL;
            if next_playout <= Instant::now() {
                next_playout = Instant::now() + FRAME_INTERVAL;
            }
        }

        if sent < FRAMES && next_send < next_playout {
            std::thread::sleep(next_send.saturating_duration_since(Instant::now()));
        } else if playout_ticks < FRAMES {
            std::thread::sleep(next_playout.saturating_duration_since(Instant::now()));
        }
    }

    let peak = output.output_mut().peak;
    output.stop();
    println!(
        "paced relay run complete: sent={sent} admitted={admitted} decoded={decoded_frames} concealed={concealed} empty_ticks={empty_ticks} playout_ticks={playout_ticks} rendered_peak={peak:.4} elapsed_ms={}",
        start.elapsed().as_millis(),
    );
    Ok(())
}

#[cfg(all(windows, feature = "playback"))]
fn play() -> LabResult<()> {
    let output = sonimesh_platform::CpalOutput::new();
    let config = output.default_device_probe()?.output_config(None, 8_192)?;
    println!("Playing a quiet 1 kHz test tone on the default output for two seconds.");
    run(output, config, None)?;
    println!("loopback playback completed: {FRAMES} Opus frames");
    Ok(())
}

#[cfg(not(all(windows, feature = "playback")))]
fn play() -> LabResult<()> {
    Err("playback requires Windows and --features playback".into())
}

fn main() -> LabResult<()> {
    match parse_args(std::env::args().skip(1))? {
        Mode::Check(relay) => check(relay),
        Mode::StreamViaRelay(relay) => stream_via_relay(relay),
        Mode::Play => play(),
        Mode::Usage => {
            println!(
                "Usage: local-audio --check | --play | --check-via-relay RELAY_PORT RECEIVER_PORT | --stream-via-relay RELAY_PORT RECEIVER_PORT"
            );
            Ok(())
        }
    }
}

enum Mode {
    Check(Option<(u16, u16)>),
    StreamViaRelay((u16, u16)),
    Play,
    Usage,
}

fn parse_args(args: impl IntoIterator<Item = String>) -> LabResult<Mode> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Mode::Usage),
        [flag] if flag == "--check" => Ok(Mode::Check(None)),
        [flag] if flag == "--play" => Ok(Mode::Play),
        [flag, listen, receive] if flag == "--check-via-relay" => {
            let listen: u16 = listen.parse()?;
            let receive: u16 = receive.parse()?;
            if listen == 0 || receive == 0 || listen == receive {
                return Err("relay and receiver ports must be distinct and nonzero".into());
            }
            Ok(Mode::Check(Some((listen, receive))))
        }
        [flag, listen, receive] if flag == "--stream-via-relay" => {
            let listen: u16 = listen.parse()?;
            let receive: u16 = receive.parse()?;
            if listen == 0 || receive == 0 || listen == receive {
                return Err("relay and receiver ports must be distinct and nonzero".into());
            }
            Ok(Mode::StreamViaRelay((listen, receive)))
        }
        _ => Err(
            "usage: local-audio --check | --play | --check-via-relay RELAY_PORT RECEIVER_PORT | --stream-via-relay RELAY_PORT RECEIVER_PORT"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_opus_udp_loopback_reaches_the_render_source() {
        super::check(None).expect("loopback audio path");
    }

    #[test]
    fn relay_ports_must_be_distinct_and_nonzero() {
        fn args(values: &[&str]) -> Vec<String> {
            values.iter().map(ToString::to_string).collect()
        }
        assert!(matches!(
            super::parse_args(args(&["--check-via-relay", "39093", "39094"])),
            Ok(super::Mode::Check(Some((39093, 39094))))
        ));
        assert!(matches!(
            super::parse_args(args(&["--stream-via-relay", "39093", "39094"])),
            Ok(super::Mode::StreamViaRelay((39093, 39094)))
        ));
        for values in [
            vec!["--check-via-relay", "0", "39094"],
            vec!["--check-via-relay", "39093", "0"],
            vec!["--check-via-relay", "39093", "39093"],
            vec!["--check-via-relay", "39093"],
            vec!["--stream-via-relay", "0", "39094"],
            vec!["--stream-via-relay", "39093", "0"],
            vec!["--stream-via-relay", "39093", "39093"],
            vec!["--stream-via-relay", "39093"],
            vec!["--check", "extra"],
        ] {
            assert!(super::parse_args(args(&values)).is_err());
        }
    }
}
