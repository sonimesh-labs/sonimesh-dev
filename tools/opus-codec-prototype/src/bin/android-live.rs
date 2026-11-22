//! Loopback-only ADB audio lab; this is not the SoniMesh wire protocol.
mod common;

use std::{
    error::Error,
    io::{self, Read},
    net::TcpListener,
    sync::Arc,
    time::Duration,
};

use sonimesh_app::{ReceiverPcmEndpointFactory, SourcePcmFormat};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{OutputConfig, RenderSource, SampleFormat};

const SAMPLES: usize = 1_920;
type LabResult<T> = Result<T, Box<dyn Error>>;

struct Stats {
    count: u64,
    peak: f32,
    render_peak: f32,
}

fn read_packet(reader: &mut impl Read, packet: &mut [u8]) -> io::Result<Option<usize>> {
    let mut prefix = [0_u8; 4];
    if reader.read(&mut prefix[..1])? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut prefix[1..])?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > packet.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid packet length",
        ));
    }
    reader.read_exact(&mut packet[..len])?;
    Ok(Some(len))
}

fn receive(
    mut stream: impl Read,
    config: OutputConfig,
    start: impl FnOnce(Box<dyn RenderSource>) -> LabResult<Option<Box<dyn RenderSource>>>,
) -> LabResult<Stats> {
    let factory = ReceiverPcmEndpointFactory::<SAMPLES>::new(
        8,
        SourcePcmFormat::new(48_000, 2)?,
        Arc::new(StreamCounters::default()),
    );
    let (mut producer, source) = factory.create(&config)?.into_parts();
    let mut check_source = start(source)?;
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Stereo)?;
    let mut packet = [0_u8; 1_148];
    let mut pcm = [0_f32; SAMPLES];
    let mut rendered = [0_f32; SAMPLES];
    let mut peak = 0_f32;
    let mut render_peak = 0_f32;
    let mut count = 0_u64;
    while let Some(len) = read_packet(&mut stream, &mut packet)? {
        let frames = decoder.decode_float(&packet[..len], &mut pcm, false)?;
        if frames != 960 {
            return Err("expected a 20 ms stereo Opus packet".into());
        }
        for &sample in &pcm {
            if !sample.is_finite() {
                return Err("non-finite decoded sample".into());
            }
            peak = peak.max(sample.abs());
        }
        producer.push(&pcm);
        if let Some(source) = check_source.as_mut() {
            source.render(&mut rendered);
            for &sample in &rendered {
                render_peak = render_peak.max(sample.abs());
            }
        }
        count += 1;
        if count % 50 == 0 {
            println!("received {count} frames; decoded peak {peak:.3}");
        }
    }
    println!("stream ended: {count} frames, decoded peak {peak:.3}");
    if check_source.is_some() {
        println!("check-render peak {render_peak:.3}");
    }
    if count == 0 {
        return Err("stream ended without audio frames".into());
    }
    if peak < 0.001 {
        println!("No capturable audio detected; the source may be silent or capture-limited.");
    }
    Ok(Stats {
        count,
        peak,
        render_peak,
    })
}

fn main() -> LabResult<()> {
    match common::parse_args(std::env::args().skip(1))? {
        common::Mode::Check => check(accept_stream()?),
        common::Mode::Play { select_output } => play(select_output),
    }
}

fn accept_stream() -> LabResult<std::net::TcpStream> {
    let listener = TcpListener::bind("127.0.0.1:39091")?;
    println!("Listening on desktop loopback port 39091; start capture on Android.");
    let (stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

fn check(stream: impl Read) -> LabResult<()> {
    let config = OutputConfig {
        sample_rate_hz: 48_000,
        channels: 2,
        sample_format: SampleFormat::Float32,
        requested_callback_frames: None,
        maximum_callback_frames: 960,
    };
    let stats = receive(stream, config, |source| Ok(Some(source)))?;
    if stats.count > 0 && stats.peak >= 0.001 && stats.render_peak < 0.001 {
        return Err("non-silent audio did not reach the render source".into());
    }
    Ok(())
}

#[cfg(all(windows, feature = "playback"))]
fn play(select_output: bool) -> LabResult<()> {
    use sonimesh_platform::AudioOutput;
    let (mut output, selection) = common::prepare_output(select_output)?;
    let stream = accept_stream()?;
    let config = selection.config;
    let result = receive(stream, config, |source| {
        output.start(selection.device.as_ref(), config, source)?;
        Ok(None)
    });
    std::thread::sleep(Duration::from_millis(160));
    let status = output.status();
    output.stop();
    println!(
        "output callbacks: {}, stream errors: {}",
        status.callback_count, status.stream_errors
    );
    result?;
    if status.callback_count == 0 || status.stream_errors > 0 || status.oversized_callbacks > 0 {
        return Err("output did not render cleanly".into());
    }
    Ok(())
}

#[cfg(not(all(windows, feature = "playback")))]
fn play(_: bool) -> LabResult<()> {
    Err("playback requires Windows and --features playback".into())
}

#[cfg(test)]
mod tests {
    use super::{SAMPLES, read_packet, receive};
    use sonimesh_platform::{OutputConfig, SampleFormat};
    use std::io::Cursor;

    #[test]
    fn framing_handles_eof_truncation_and_limits() {
        let mut packet = [0; 4];
        let mut source = Cursor::new([0, 0, 0, 2, 8, 9]);
        assert_eq!(read_packet(&mut source, &mut packet).unwrap(), Some(2));
        assert_eq!(&packet[..2], &[8, 9]);
        assert_eq!(read_packet(&mut source, &mut packet).unwrap(), None);
        assert!(read_packet(&mut Cursor::new([0, 0]), &mut packet).is_err());
        assert!(read_packet(&mut Cursor::new([0, 0, 0, 2, 8]), &mut packet).is_err());
        assert!(read_packet(&mut Cursor::new([0, 0, 0, 5]), &mut packet).is_err());
        assert!(read_packet(&mut Cursor::new([0, 0, 0, 0]), &mut packet).is_err());
    }

    #[test]
    fn encoded_tone_reaches_the_render_source() {
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut pcm = [0_f32; SAMPLES];
        for (index, stereo) in pcm.chunks_exact_mut(2).enumerate() {
            let phase = u16::try_from(index % 48).unwrap();
            stereo.fill((f32::from(phase) * std::f32::consts::TAU / 48.0).sin() * 0.1);
        }
        let mut packet = [0; 1_148];
        let mut data = Vec::new();
        for _ in 0..10 {
            let len = encoder.encode_float(&pcm, &mut packet).unwrap();
            data.extend_from_slice(&u32::try_from(len).unwrap().to_be_bytes());
            data.extend_from_slice(&packet[..len]);
        }
        let config = OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: None,
            maximum_callback_frames: 960,
        };
        let stats = receive(Cursor::new(data), config, |source| Ok(Some(source))).unwrap();
        assert_eq!(stats.count, 10);
        assert!(stats.peak > 0.01 && stats.render_peak > 0.01);
    }
}
