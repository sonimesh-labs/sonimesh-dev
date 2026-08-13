use cpal::traits::DeviceTrait;

use crate::{OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, SampleFormat};

/// Buffer-size information exposed by CPAL for one default output format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpalBufferSize {
    /// Backend does not publish a fixed range before stream creation.
    Unknown,
    /// Inclusive supported range reported by the backend.
    Range {
        /// Smallest reported callback request.
        minimum_frames: u32,
        /// Largest reported callback request.
        maximum_frames: u32,
    },
}

/// Control-domain snapshot of one CPAL output endpoint and its default format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpalDeviceProbe {
    /// Persistent ID and untrusted display metadata.
    pub device: OutputDeviceInfo,
    /// Default device sample rate.
    pub sample_rate_hz: u32,
    /// Default interleaved channel count.
    pub channels: u16,
    /// SoniMesh format when the default backend format is currently supported.
    pub sample_format: Option<SampleFormat>,
    /// Backend format label retained for feasibility reports.
    pub backend_sample_format: String,
    /// Buffer request range exposed by CPAL.
    pub buffer_size: CpalBufferSize,
}

pub(crate) fn device_info(
    device: &cpal::Device,
    default_id: Option<&str>,
) -> Result<OutputDeviceInfo, cpal::Error> {
    let id = device.id()?.to_string();
    Ok(OutputDeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id: OutputDeviceId::new(id),
        display_name: device.to_string(),
    })
}

impl CpalDeviceProbe {
    /// Creates a bounded SoniMesh configuration from the probed default format.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::UnsupportedFormat`] for a backend sample type not
    /// supported by the MVP, or [`OutputError::InvalidConfig`] for invalid bounds.
    pub fn output_config(
        &self,
        requested_callback_frames: Option<u32>,
        maximum_callback_frames: u32,
    ) -> Result<OutputConfig, OutputError> {
        let sample_format = self.sample_format.ok_or(OutputError::UnsupportedFormat)?;
        let config = OutputConfig {
            sample_rate_hz: self.sample_rate_hz,
            channels: self.channels,
            sample_format,
            requested_callback_frames,
            maximum_callback_frames,
        };
        config.validate().map_err(OutputError::InvalidConfig)?;
        Ok(config)
    }
}

pub(crate) fn probe_device(
    device: &cpal::Device,
    default_id: Option<&str>,
) -> Result<CpalDeviceProbe, cpal::Error> {
    let device_info = device_info(device, default_id)?;
    let default = device.default_output_config()?;
    let sample_format = match default.sample_format() {
        cpal::SampleFormat::F32 => Some(SampleFormat::Float32),
        cpal::SampleFormat::I16 => Some(SampleFormat::Signed16),
        _ => None,
    };
    let buffer_size = match *default.buffer_size() {
        cpal::SupportedBufferSize::Unknown => CpalBufferSize::Unknown,
        cpal::SupportedBufferSize::Range { min, max } => CpalBufferSize::Range {
            minimum_frames: min,
            maximum_frames: max,
        },
    };

    Ok(CpalDeviceProbe {
        device: device_info,
        sample_rate_hz: default.sample_rate(),
        channels: default.channels(),
        sample_format,
        backend_sample_format: format!("{:?}", default.sample_format()),
        buffer_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(sample_format: Option<SampleFormat>) -> CpalDeviceProbe {
        CpalDeviceProbe {
            device: OutputDeviceInfo {
                id: OutputDeviceId::new("wasapi:test"),
                display_name: "Test device".to_owned(),
                is_default: true,
            },
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format,
            backend_sample_format: "F32".to_owned(),
            buffer_size: CpalBufferSize::Unknown,
        }
    }

    #[test]
    fn builds_config_from_supported_default_format() {
        let config = probe(Some(SampleFormat::Float32))
            .output_config(Some(480), 1_024)
            .unwrap();

        assert_eq!(config.sample_rate_hz, 48_000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.requested_callback_frames, Some(480));
        assert_eq!(config.maximum_callback_frames, 1_024);
    }

    #[test]
    fn rejects_default_format_outside_mvp_set() {
        assert_eq!(
            probe(None).output_config(None, 1_024),
            Err(OutputError::UnsupportedFormat)
        );
    }

    #[test]
    fn output_config_wraps_invalid_probe_bounds() {
        let mut probe = probe(Some(SampleFormat::Float32));
        probe.sample_rate_hz = 0;
        assert!(matches!(
            probe.output_config(None, 1_024),
            Err(OutputError::InvalidConfig(_))
        ));
    }

    #[test]
    fn buffer_size_variants_compare_and_debug_cleanly() {
        assert_eq!(CpalBufferSize::Unknown, CpalBufferSize::Unknown);
        let range = CpalBufferSize::Range {
            minimum_frames: 240,
            maximum_frames: 4_800,
        };
        assert_ne!(range, CpalBufferSize::Unknown);
        assert_eq!(
            format!("{range:?}"),
            "Range { minimum_frames: 240, maximum_frames: 4800 }"
        );
    }
}
