use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    endpoint_name: String,
    sample_rate: u32,
    channels: u16,
    samples: u64,
    peak: f32,
    error: Option<String>,
}

#[cfg(windows)]
mod platform {
    use super::Snapshot;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, SizedSample, Stream, StreamConfig};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    };
    use std::time::Duration;

    #[derive(Default)]
    struct Meter {
        samples: AtomicU64,
        peak_bits: AtomicU32,
        error: AtomicBool,
    }

    pub struct Monitor {
        stream: Option<Stream>,
        meter: Arc<Meter>,
        endpoint_name: String,
        sample_rate: u32,
        channels: u16,
    }

    impl Default for Monitor {
        fn default() -> Self {
            Self {
                stream: None,
                meter: Arc::new(Meter::default()),
                endpoint_name: String::new(),
                sample_rate: 0,
                channels: 0,
            }
        }
    }

    fn build_stream<T: SizedSample + Copy + 'static>(
        device: &cpal::Device,
        config: &StreamConfig,
        meter: Arc<Meter>,
        convert: fn(T) -> f32,
    ) -> Result<Stream, String> {
        let callback_meter = Arc::clone(&meter);
        device
            .build_input_stream(
                *config,
                move |data: &[T], _| {
                    let mut peak = 0.0_f32;
                    for sample in data {
                        peak = peak.max(convert(*sample).abs());
                    }
                    callback_meter
                        .samples
                        .fetch_add(data.len() as u64, Ordering::Relaxed);
                    callback_meter
                        .peak_bits
                        .fetch_max(peak.to_bits(), Ordering::Relaxed);
                },
                move |_| {
                    meter.error.store(true, Ordering::Relaxed);
                },
                Some(Duration::from_secs(3)),
            )
            .map_err(|error| error.to_string())
    }

    impl Monitor {
        pub fn start(&mut self) -> Result<Snapshot, String> {
            if self.stream.is_some() {
                return Ok(self.snapshot());
            }
            let device = cpal::default_host()
                .default_output_device()
                .ok_or("No default Windows playback device is available.")?;
            let endpoint_name = device
                .description()
                .map_err(|error| error.to_string())?
                .name()
                .to_owned();
            let supported = device
                .default_output_config()
                .map_err(|error| error.to_string())?;
            let sample_rate = supported.sample_rate();
            let channels = supported.channels();
            let config: StreamConfig = supported.config();
            let meter = Arc::new(Meter::default());
            let stream = match supported.sample_format() {
                SampleFormat::F32 => {
                    build_stream::<f32>(&device, &config, Arc::clone(&meter), |sample| sample)
                }
                SampleFormat::I16 => {
                    build_stream::<i16>(&device, &config, Arc::clone(&meter), |sample| {
                        sample as f32 / i16::MAX as f32
                    })
                }
                SampleFormat::U16 => {
                    build_stream::<u16>(&device, &config, Arc::clone(&meter), |sample| {
                        (sample as f32 - 32768.0) / 32768.0
                    })
                }
                format => Err(format!("Unsupported loopback sample format: {format:?}")),
            }?;
            stream.play().map_err(|error| error.to_string())?;
            self.stream = Some(stream);
            self.meter = meter;
            self.endpoint_name = endpoint_name;
            self.sample_rate = sample_rate;
            self.channels = channels;
            Ok(self.snapshot())
        }

        pub fn stop(&mut self) -> Snapshot {
            self.stream = None;
            self.snapshot()
        }

        pub fn snapshot(&self) -> Snapshot {
            let error = self
                .meter
                .error
                .load(Ordering::Relaxed)
                .then(|| "The Windows loopback stream reported an error.".to_owned());
            Snapshot {
                running: self.stream.is_some(),
                endpoint_name: self.endpoint_name.clone(),
                sample_rate: self.sample_rate,
                channels: self.channels,
                samples: self.meter.samples.load(Ordering::Relaxed),
                peak: f32::from_bits(self.meter.peak_bits.swap(0, Ordering::Relaxed)),
                error,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::Meter;
        use std::sync::atomic::Ordering;

        #[test]
        fn peak_is_reset_after_each_snapshot() {
            let meter = Meter::default();
            meter
                .peak_bits
                .fetch_max(0.75_f32.to_bits(), Ordering::Relaxed);
            meter
                .peak_bits
                .fetch_max(0.25_f32.to_bits(), Ordering::Relaxed);

            assert_eq!(
                f32::from_bits(meter.peak_bits.swap(0, Ordering::Relaxed)),
                0.75
            );
            assert_eq!(
                f32::from_bits(meter.peak_bits.swap(0, Ordering::Relaxed)),
                0.0
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Snapshot;

    #[derive(Default)]
    pub struct Monitor;

    impl Monitor {
        pub fn start(&mut self) -> Result<Snapshot, String> {
            Err("System output loopback is available only in the Windows probe.".into())
        }

        pub fn stop(&mut self) -> Snapshot {
            Snapshot {
                running: false,
                endpoint_name: String::new(),
                sample_rate: 0,
                channels: 0,
                samples: 0,
                peak: 0.0,
                error: None,
            }
        }

        pub fn snapshot(&self) -> Snapshot {
            Snapshot {
                running: false,
                endpoint_name: String::new(),
                sample_rate: 0,
                channels: 0,
                samples: 0,
                peak: 0.0,
                error: None,
            }
        }
    }
}

pub use platform::Monitor;
