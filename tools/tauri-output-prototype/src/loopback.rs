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
    run_peak: f32,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    id: String,
    name: String,
    is_default: bool,
    sample_rate: Option<u32>,
    channels: Option<u16>,
}

fn sort_endpoints(endpoints: &mut [Endpoint]) {
    endpoints.sort_by_key(|endpoint| {
        (
            !endpoint.is_default,
            endpoint.name.to_lowercase(),
            endpoint.id.clone(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, sort_endpoints};

    fn endpoint(id: &str, name: &str, is_default: bool) -> Endpoint {
        Endpoint {
            id: id.into(),
            name: name.into(),
            is_default,
            sample_rate: None,
            channels: None,
        }
    }

    #[test]
    fn output_endpoints_sort_default_first_then_by_name() {
        let mut endpoints = [
            endpoint("z", "Speakers", false),
            endpoint("b", "Bluetooth Headphones", false),
            endpoint("a", "Aux", true),
            endpoint("c", "bluetooth headphones", false),
        ];

        sort_endpoints(&mut endpoints);

        assert_eq!(
            endpoints
                .iter()
                .map(|endpoint| endpoint.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c", "z"]
        );
    }
}

#[cfg(windows)]
mod platform {
    use super::{Endpoint, Snapshot, sort_endpoints};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{DeviceId, FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
    use std::str::FromStr;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    };
    use std::time::Duration;

    #[derive(Default)]
    struct Meter {
        samples: AtomicU64,
        peak_bits: AtomicU32,
        run_peak_bits: AtomicU32,
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
    ) -> Result<Stream, String>
    where
        f32: FromSample<T>,
    {
        let callback_meter = Arc::clone(&meter);
        device
            .build_input_stream(
                *config,
                move |data: &[T], _| {
                    let mut peak = 0.0_f32;
                    for sample in data {
                        peak = peak.max(sample.to_sample::<f32>().abs());
                    }
                    callback_meter
                        .samples
                        .fetch_add(data.len() as u64, Ordering::Relaxed);
                    callback_meter
                        .peak_bits
                        .fetch_max(peak.to_bits(), Ordering::Relaxed);
                    callback_meter
                        .run_peak_bits
                        .fetch_max(peak.to_bits(), Ordering::Relaxed);
                },
                move |_| {
                    meter.error.store(true, Ordering::Relaxed);
                },
                Some(Duration::from_secs(3)),
            )
            .map_err(|error| error.to_string())
    }

    pub fn endpoints() -> Result<Vec<Endpoint>, String> {
        let host = cpal::default_host();
        let default_id = host
            .default_output_device()
            .and_then(|device| device.id().ok());
        let mut endpoints: Vec<_> = host
            .output_devices()
            .map_err(|error| error.to_string())?
            .map(|device| {
                let id = device.id().map_err(|error| error.to_string())?;
                let name = device
                    .description()
                    .map_err(|error| error.to_string())?
                    .name()
                    .to_owned();
                let format = device.default_output_config().ok();
                Ok(Endpoint {
                    is_default: default_id.as_ref() == Some(&id),
                    id: id.to_string(),
                    name,
                    sample_rate: format.as_ref().map(|config| config.sample_rate()),
                    channels: format.map(|config| config.channels()),
                })
            })
            .collect::<Result<_, String>>()?;
        sort_endpoints(&mut endpoints);
        Ok(endpoints)
    }

    impl Monitor {
        pub fn start(&mut self, endpoint_id: &str) -> Result<Snapshot, String> {
            if self.stream.is_some() {
                return Ok(self.snapshot());
            }
            let host = cpal::default_host();
            let device_id = DeviceId::from_str(endpoint_id).map_err(|error| error.to_string())?;
            let device = host
                .device_by_id(&device_id)
                .ok_or("The selected Windows playback device is no longer available.")?;
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
                SampleFormat::I8 => build_stream::<i8>(&device, &config, Arc::clone(&meter)),
                SampleFormat::I16 => build_stream::<i16>(&device, &config, Arc::clone(&meter)),
                SampleFormat::I24 => {
                    build_stream::<cpal::I24>(&device, &config, Arc::clone(&meter))
                }
                SampleFormat::I32 => build_stream::<i32>(&device, &config, Arc::clone(&meter)),
                SampleFormat::I64 => build_stream::<i64>(&device, &config, Arc::clone(&meter)),
                SampleFormat::U8 => build_stream::<u8>(&device, &config, Arc::clone(&meter)),
                SampleFormat::U16 => build_stream::<u16>(&device, &config, Arc::clone(&meter)),
                SampleFormat::U24 => {
                    build_stream::<cpal::U24>(&device, &config, Arc::clone(&meter))
                }
                SampleFormat::U32 => build_stream::<u32>(&device, &config, Arc::clone(&meter)),
                SampleFormat::U64 => build_stream::<u64>(&device, &config, Arc::clone(&meter)),
                SampleFormat::F32 => build_stream::<f32>(&device, &config, Arc::clone(&meter)),
                SampleFormat::F64 => build_stream::<f64>(&device, &config, Arc::clone(&meter)),
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
                run_peak: f32::from_bits(self.meter.run_peak_bits.load(Ordering::Relaxed)),
                error,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Meter, Monitor, endpoints};
        use cpal::traits::HostTrait;
        use cpal::{FromSample, Sample};
        use std::str::FromStr;
        use std::sync::atomic::Ordering;

        fn normalized<T: Sample>(sample: T) -> f32
        where
            f32: FromSample<T>,
        {
            sample.to_sample::<f32>()
        }

        #[test]
        fn sample_formats_normalize_to_float_peak_range() {
            assert_eq!(normalized(i8::MAX), 127.0 / 128.0);
            assert_eq!(normalized(u8::MAX), 127.0 / 128.0);
            assert_eq!(normalized(0_u16), -1.0);
            assert_eq!(normalized(0.5_f64), 0.5);
        }

        #[test]
        #[ignore = "requires Windows playback endpoints"]
        fn listed_output_endpoints_can_be_reopened_by_id() {
            let endpoints = endpoints().unwrap();
            assert!(!endpoints.is_empty());
            if let Some(default_index) = endpoints.iter().position(|endpoint| endpoint.is_default) {
                assert_eq!(default_index, 0, "default endpoint should be listed first");
            }
            let host = cpal::default_host();
            for endpoint in endpoints {
                eprintln!(
                    "endpoint={} default={} sample_rate_hz={:?} channels={:?}",
                    endpoint.name, endpoint.is_default, endpoint.sample_rate, endpoint.channels
                );
                let id = cpal::DeviceId::from_str(&endpoint.id).unwrap();
                assert!(host.device_by_id(&id).is_some(), "{}", endpoint.name);
            }
        }

        #[test]
        #[ignore = "starts WASAPI loopback on a Windows playback endpoint"]
        fn selected_default_endpoint_loopback_starts_and_stops() {
            let endpoint = endpoints()
                .unwrap()
                .into_iter()
                .find(|endpoint| endpoint.is_default)
                .expect("no default Windows playback endpoint is available");
            let mut monitor = Monitor::default();
            let started = monitor
                .start(&endpoint.id)
                .expect("could not start selected endpoint loopback");
            assert!(started.running);
            assert_eq!(started.endpoint_name, endpoint.name);
            assert!(!monitor.stop().running);
        }

        #[test]
        #[ignore = "requires a Windows Bluetooth candidate and playback endpoints"]
        fn bluetooth_wait_and_default_endpoint_monitor_run_together() {
            use crate::bluetooth::{Bluetooth, PlaybackState};

            let mut bluetooth = Bluetooth::default();
            bluetooth
                .refresh()
                .expect("could not refresh Bluetooth sources");
            let endpoint = endpoints()
                .unwrap()
                .into_iter()
                .find(|endpoint| endpoint.is_default)
                .expect("no default Windows playback endpoint is available");
            let mut monitor = Monitor::default();

            assert!(
                monitor.start(&endpoint.id).unwrap().running,
                "default endpoint monitoring did not start"
            );
            bluetooth
                .prepare(1, 0)
                .expect("could not prepare the Bluetooth wait");
            assert!(matches!(
                bluetooth.status().unwrap(),
                PlaybackState::Waiting | PlaybackState::Open
            ));

            bluetooth.disconnect().unwrap();
            assert_eq!(bluetooth.status().unwrap(), PlaybackState::Idle);
            assert!(!monitor.stop().running);
        }

        #[test]
        #[ignore = "requires an available non-default Windows playback endpoint"]
        fn selected_nondefault_endpoint_loopback_starts_and_stops() {
            let endpoint = endpoints()
                .unwrap()
                .into_iter()
                .find(|endpoint| !endpoint.is_default)
                .expect("no non-default Windows playback endpoint is available");
            let mut monitor = Monitor::default();
            let started = monitor
                .start(&endpoint.id)
                .expect("could not start selected non-default endpoint loopback");
            assert!(started.running);
            assert_eq!(started.endpoint_name, endpoint.name);
            assert!(!monitor.stop().running);
        }

        #[test]
        fn peak_is_reset_after_each_snapshot() {
            let meter = Meter::default();
            meter
                .peak_bits
                .fetch_max(0.75_f32.to_bits(), Ordering::Relaxed);
            meter
                .run_peak_bits
                .fetch_max(0.75_f32.to_bits(), Ordering::Relaxed);
            meter
                .peak_bits
                .fetch_max(0.25_f32.to_bits(), Ordering::Relaxed);
            meter
                .run_peak_bits
                .fetch_max(0.25_f32.to_bits(), Ordering::Relaxed);

            assert_eq!(
                f32::from_bits(meter.peak_bits.swap(0, Ordering::Relaxed)),
                0.75
            );
            assert_eq!(
                f32::from_bits(meter.peak_bits.swap(0, Ordering::Relaxed)),
                0.0
            );
            assert_eq!(
                f32::from_bits(meter.run_peak_bits.load(Ordering::Relaxed)),
                0.75
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Endpoint, Snapshot};

    pub fn endpoints() -> Result<Vec<Endpoint>, String> {
        Ok(Vec::new())
    }

    #[derive(Default)]
    pub struct Monitor;

    impl Monitor {
        pub fn start(&mut self, _endpoint_id: &str) -> Result<Snapshot, String> {
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
                run_peak: 0.0,
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
                run_peak: 0.0,
                error: None,
            }
        }
    }
}

pub use platform::Monitor;
pub use platform::endpoints;
