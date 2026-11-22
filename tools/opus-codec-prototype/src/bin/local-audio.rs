//! Loopback-only Opus-to-output lab. The fixed test key is not a pairing key.

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sonimesh_app::{
    OutputSelection, ReceiverOutputController, ReceiverPcmEndpointFactory, SourcePcmFormat,
    UdpPollOutcome, UdpReceiverPipeline,
};
use sonimesh_audio::{
    DecodeStep, FrameDecoder, OpusFrameConfig, StreamDecodeWorker, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{MediaSessionSendOutcome, MediaSessionSender};
use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

const SAMPLES: usize = 1_920;
const FRAMES: u32 = 100;
type LabResult<T> = Result<T, Box<dyn Error>>;

struct Decoder(opus::Decoder);

impl FrameDecoder for Decoder {
    type Error = opus::Error;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        self.0.decode_float(encoded, output, false).map(|n| n * 2)
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

fn run<O: LabOutput>(output: O, config: OutputConfig) -> LabResult<()> {
    let frame = OpusFrameConfig::new(48_000, 2, 20)?;
    let secret = SessionSecret::new([0x42; 32]);
    let context = MediaKeyContext::new(1, 1, 0, MediaDirection::AndroidToDesktop)?;
    let counters = Arc::new(StreamCounters::default());
    let (producer, _unused_consumer) = realtime_pcm_queue::<SAMPLES>(8)?;
    let decoder = Decoder(opus::Decoder::new(48_000, opus::Channels::Stereo)?);
    let worker =
        StreamDecodeWorker::<_, 8, 1_148, SAMPLES>::new_for_opus(decoder, producer, 0, frame)?;
    let mut receiver = UdpReceiverPipeline::bind_with_worker(
        "127.0.0.1:0".parse()?,
        &secret,
        context,
        worker,
        Arc::clone(&counters),
    )?;
    let mut sender = MediaSessionSender::bind(
        "127.0.0.1:0".parse()?,
        &secret,
        context,
        receiver.local_addr()?,
    )?;
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

fn check() -> LabResult<()> {
    let config = OutputConfig {
        sample_rate_hz: 48_000,
        channels: 2,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: None,
        maximum_callback_frames: 960,
    };
    run(CheckOutput::default(), config)?;
    println!("loopback audio check passed: {FRAMES} Opus frames, non-silent render");
    Ok(())
}

#[cfg(all(windows, feature = "playback"))]
fn play() -> LabResult<()> {
    let output = sonimesh_platform::CpalOutput::new();
    let config = output.default_device_probe()?.output_config(None, 8_192)?;
    println!("Playing a quiet 1 kHz test tone on the default output for two seconds.");
    run(output, config)?;
    println!("loopback playback completed: {FRAMES} Opus frames");
    Ok(())
}

#[cfg(not(all(windows, feature = "playback")))]
fn play() -> LabResult<()> {
    Err("playback requires Windows and --features playback".into())
}

fn main() -> LabResult<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--check") => check(),
        Some("--play") => play(),
        _ => {
            println!("Usage: local-audio --check | --play");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_opus_udp_loopback_reaches_the_render_source() {
        super::check().expect("loopback audio path");
    }
}
