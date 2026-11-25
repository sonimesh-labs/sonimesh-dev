use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, SampleFormat, SizedSample, Stream};
use sonimesh_audio::{RealtimePcmProducer, realtime_pcm_queue};
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

const BLOCK_SAMPLES: usize = 1_920;
const QUEUE_BLOCKS: usize = 8;

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
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: RealtimePcmProducer<BLOCK_SAMPLES>,
    channels: usize,
    stats: Arc<CaptureStats>,
) -> Result<Stream, Box<dyn Error>>
where
    T: SizedSample + Copy + 'static,
    f32: FromSample<T>,
{
    let mut block = [0.0_f32; BLOCK_SAMPLES];
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
                if offset == BLOCK_SAMPLES {
                    let _ = producer.push(&block);
                    offset = 0;
                    callback_stats
                        .samples
                        .fetch_add(BLOCK_SAMPLES as u64, Ordering::Relaxed);
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
        if supported.sample_rate() != 48_000 {
            return Err(format!(
                "selected endpoint uses {} Hz; this lab currently requires 48 kHz",
                supported.sample_rate()
            )
            .into());
        }
        let channels = usize::from(supported.channels());
        if channels == 0 {
            return Err("selected endpoint has no audio channels".into());
        }
        let config = supported.config();
        let (producer, consumer) = realtime_pcm_queue::<BLOCK_SAMPLES>(QUEUE_BLOCKS)?;
        let stats = Arc::new(CaptureStats::default());
        let stream = match supported.sample_format() {
            SampleFormat::I8 => {
                build_stream::<i8>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::I24 => {
                build_stream::<cpal::I24>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::I32 => {
                build_stream::<i32>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::I64 => {
                build_stream::<i64>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::U8 => {
                build_stream::<u8>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::U24 => {
                build_stream::<cpal::U24>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::U32 => {
                build_stream::<u32>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::U64 => {
                build_stream::<u64>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            SampleFormat::F64 => {
                build_stream::<f64>(&device, &config, producer, channels, Arc::clone(&stats))
            }
            format => return Err(format!("unsupported loopback sample format: {format:?}").into()),
        }?;
        stream.play()?;
        Ok(Self {
            _stream: stream,
            consumer,
            stats,
            endpoint_name,
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
        assert_eq!(stereo_pair(0.25, None), [0.25, 0.25]);
        assert_eq!(stereo_pair(f32::NAN, Some(0.5)), [0.0, 0.5]);
        assert_eq!(stereo_pair(0.25, Some(f32::INFINITY)), [0.25, 0.0]);
    }

    #[test]
    fn block_size_matches_one_stereo_opus_frame() {
        assert_eq!(BLOCK_SAMPLES, 48_000 / 50 * 2);
    }

    #[test]
    fn integer_and_float_samples_convert_to_float() {
        assert_eq!(i16::MAX.to_sample::<f32>(), 32767.0 / 32768.0);
        assert_eq!(0.5_f64.to_sample::<f32>(), 0.5);
    }

    #[test]
    fn xrun_is_counted_without_stopping_capture() {
        let stats = CaptureStats::default();
        record_stream_error(&stats, &ErrorKind::Xrun.into());

        assert_eq!(stats.xruns.load(Ordering::Relaxed), 1);
        assert!(!stats.error.load(Ordering::Relaxed));
    }
}
