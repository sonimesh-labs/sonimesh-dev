use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, Stream};
use sonimesh_audio::{RealtimePcmProducer, realtime_pcm_queue};
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

pub const BLOCK_SAMPLES: usize = 7_680;
const QUEUE_BLOCKS: usize = 8;
const CAPTURE_BLOCK_DURATION_MS: u32 = 20;
const MIN_CAPTURE_RATE_HZ: u32 = 8_000;
const MAX_CAPTURE_RATE_HZ: u32 = 192_000;

fn samples_per_capture_block(sample_rate_hz: u32) -> Option<usize> {
    if !(MIN_CAPTURE_RATE_HZ..=MAX_CAPTURE_RATE_HZ).contains(&sample_rate_hz)
        || sample_rate_hz % (1_000 / CAPTURE_BLOCK_DURATION_MS) != 0
    {
        return None;
    }
    usize::try_from(sample_rate_hz / (1_000 / CAPTURE_BLOCK_DURATION_MS))
        .ok()?
        .checked_mul(2)
}

fn stereo_pair(left: f32, right: Option<f32>) -> [f32; 2] {
    let left = if left.is_finite() { left } else { 0.0 };
    let right = right.unwrap_or(left);
    let right = if right.is_finite() { right } else { 0.0 };
    [left, right]
}

#[derive(Default)]
struct CaptureStats {
    samples: AtomicU64,
    peak_bits: AtomicU32,
    xruns: AtomicU64,
    error: AtomicBool,
}

fn record_stream_error(stats: &CaptureStats, error: &cpal::Error) {
    if error.kind() == ErrorKind::Xrun {
        stats.xruns.fetch_add(1, Ordering::Relaxed);
    } else {
        eprintln!("Windows loopback input stream failed: {error}");
        stats.error.store(true, Ordering::Relaxed);
    }
}

pub struct Capture {
    _stream: Stream,
    pub consumer: sonimesh_audio::RealtimePcmConsumer<BLOCK_SAMPLES>,
    stats: Arc<CaptureStats>,
    pub endpoint_name: String,
    pub sample_rate_hz: u32,
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: RealtimePcmProducer<BLOCK_SAMPLES>,
    channels: usize,
    samples_per_block: usize,
    stats: Arc<CaptureStats>,
) -> Result<Stream, Box<dyn Error>>
where
    T: SizedSample + Copy + 'static,
    f32: FromSample<T>,
{
    let mut block = vec![0.0_f32; BLOCK_SAMPLES].into_boxed_slice();
    let mut offset = 0;
    let callback_stats = Arc::clone(&stats);
    Ok(device.build_input_stream(
        *config,
        move |data: &[T], _| {
            let mut peak = 0.0_f32;
            for frame in data.chunks_exact(channels) {
                let left = frame[0].to_sample::<f32>();
                let right = if channels == 1 {
                    None
                } else {
                    Some(frame[1].to_sample::<f32>())
                };
                let [left, right] = stereo_pair(left, right);
                peak = peak.max(left.abs()).max(right.abs());
                block[offset] = left;
                block[offset + 1] = right;
                offset += 2;
                if offset == samples_per_block {
                    let _ = producer.push(&block[..samples_per_block]);
                    offset = 0;
                    callback_stats
                        .samples
                        .fetch_add(samples_per_block as u64, Ordering::Relaxed);
                }
            }
            callback_stats
                .peak_bits
                .fetch_max(peak.to_bits(), Ordering::Relaxed);
        },
        move |error| record_stream_error(&stats, &error),
        Some(Duration::from_secs(3)),
    )?)
}

fn build_sample_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    producer: RealtimePcmProducer<BLOCK_SAMPLES>,
    channels: usize,
    samples_per_block: usize,
    stats: &Arc<CaptureStats>,
    format: SampleFormat,
) -> Result<Stream, Box<dyn Error>> {
    macro_rules! build {
        ($sample:ty) => {
            build_stream::<$sample>(
                device,
                config,
                producer,
                channels,
                samples_per_block,
                Arc::clone(stats),
            )
        };
    }
    match format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U24 => build!(cpal::U24),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        format => Err(format!("unsupported loopback sample format: {format:?}").into()),
    }
}

impl Capture {
    pub fn list_endpoints() -> Result<(), Box<dyn Error>> {
        let host = cpal::default_host();
        let default_id = host
            .default_output_device()
            .and_then(|device| device.id().ok());
        for device in host.output_devices()? {
            let id = device.id()?;
            let name = device.description()?.name().to_owned();
            let is_default = default_id.as_ref() == Some(&id);
            match device.default_output_config() {
                Ok(config) => println!(
                    "{} | id={} | {} | {} Hz | {} ch | {:?}",
                    if is_default { "default" } else { "output" },
                    id,
                    name,
                    config.sample_rate(),
                    config.channels(),
                    config.sample_format()
                ),
                Err(error) => println!(
                    "{} | id={} | {} | format unavailable: {error}",
                    if is_default { "default" } else { "output" },
                    id,
                    name,
                ),
            }
        }
        Ok(())
    }

    pub fn start(endpoint_id: Option<&str>) -> Result<Self, Box<dyn Error>> {
        let host = cpal::default_host();
        let device = if let Some(endpoint_id) = endpoint_id {
            let id = cpal::DeviceId::from_str(endpoint_id)?;
            host.device_by_id(&id)
                .ok_or("selected Windows output endpoint is no longer available")?
        } else {
            host.default_output_device()
                .ok_or("no default Windows output endpoint is available")?
        };
        let endpoint_name = device.description()?.name().to_owned();
        let supported = device.default_output_config()?;
        let sample_rate_hz = supported.sample_rate();
        let Some(samples_per_block) = samples_per_capture_block(sample_rate_hz) else {
            return Err(format!(
                "selected endpoint uses {sample_rate_hz} Hz; this lab supports 8-192 kHz rates with whole 20 ms blocks"
            )
            .into());
        };
        let channels = usize::from(supported.channels());
        if channels == 0 {
            return Err("selected endpoint has no audio channels".into());
        }
        let config = supported.config();
        let (producer, consumer) = realtime_pcm_queue::<BLOCK_SAMPLES>(QUEUE_BLOCKS)?;
        let stats = Arc::new(CaptureStats::default());
        let stream = build_sample_stream(
            &device,
            &config,
            producer,
            channels,
            samples_per_block,
            &stats,
            supported.sample_format(),
        )?;
        stream.play()?;
        Ok(Self {
            _stream: stream,
            consumer,
            stats,
            endpoint_name,
            sample_rate_hz,
        })
    }

    pub fn samples(&self) -> u64 {
        self.stats.samples.load(Ordering::Relaxed)
    }

    pub fn peak(&self) -> f32 {
        f32::from_bits(self.stats.peak_bits.load(Ordering::Relaxed))
    }

    pub fn xruns(&self) -> u64 {
        self.stats.xruns.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> bool {
        self.stats.error.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;

    #[test]
    fn mono_is_duplicated_and_non_finite_samples_become_silence() {
        assert_eq!(
            stereo_pair(0.25, None).map(f32::to_bits),
            [0.25, 0.25].map(f32::to_bits)
        );
        assert_eq!(
            stereo_pair(f32::NAN, Some(0.5)).map(f32::to_bits),
            [0.0, 0.5].map(f32::to_bits)
        );
        assert_eq!(
            stereo_pair(0.25, Some(f32::INFINITY)).map(f32::to_bits),
            [0.25, 0.0].map(f32::to_bits)
        );
    }

    #[test]
    fn capture_rates_fit_the_bounded_stereo_block() {
        for rate in [
            8_000, 16_000, 24_000, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
        ] {
            let samples = samples_per_capture_block(rate).unwrap();
            assert_eq!(samples, usize::try_from(rate / 50).unwrap() * 2);
            assert!(samples <= BLOCK_SAMPLES);
        }
        assert_eq!(samples_per_capture_block(7_950), None);
        assert_eq!(samples_per_capture_block(11_025), None);
        assert_eq!(samples_per_capture_block(192_050), None);
    }

    #[test]
    fn integer_and_float_samples_convert_to_float() {
        assert_eq!(
            i16::MAX.to_sample::<f32>().to_bits(),
            (32767.0_f32 / 32768.0).to_bits()
        );
        assert_eq!(0.5_f64.to_sample::<f32>().to_bits(), 0.5_f32.to_bits());
    }

    #[test]
    fn xrun_is_counted_without_stopping_capture() {
        let stats = CaptureStats::default();
        record_stream_error(&stats, &ErrorKind::Xrun.into());

        assert_eq!(stats.xruns.load(Ordering::Relaxed), 1);
        assert!(!stats.error.load(Ordering::Relaxed));
    }
}
