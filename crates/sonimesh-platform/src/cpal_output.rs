use std::str::FromStr;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::cpal_callback::{build_float32_stream, build_signed16_stream};
use crate::cpal_probe::{CpalDeviceProbe, device_info, probe_device};
use crate::cpal_status::{CallbackStatus, CpalOutputStatus};
use crate::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};

/// Optional CPAL/WASAPI output adapter for Windows feasibility measurements.
///
/// Construction and all lifecycle methods belong to the control domain. The
/// callback owns its render source and only touches preallocated memory and atoms.
pub struct CpalOutput {
    host: cpal::Host,
    stream: Option<cpal::Stream>,
    status: Arc<CallbackStatus>,
}

impl CpalOutput {
    /// Creates a stopped adapter using CPAL's default host.
    #[must_use]
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
            stream: None,
            status: Arc::new(CallbackStatus::default()),
        }
    }

    /// Reads a non-transactional low-rate callback status snapshot.
    #[must_use]
    pub fn status(&self) -> CpalOutputStatus {
        self.status.snapshot()
    }

    /// Returns whether a CPAL stream is currently owned by the adapter.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.stream.is_some()
    }

    /// Enumerates output endpoints with their current default stream formats.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when the host, an endpoint ID, or its default
    /// configuration cannot be queried.
    pub fn probe_devices(&self) -> Result<Vec<CpalDeviceProbe>, OutputError> {
        let default_id = self.default_device_id();
        self.host
            .output_devices()
            .map_err(|error| map_cpal_error(&error))?
            .map(|device| {
                probe_device(&device, default_id.as_deref()).map_err(|error| map_cpal_error(&error))
            })
            .collect()
    }

    /// Probes the endpoint currently selected by the Windows default role.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::DeviceUnavailable`] when no default exists, or a
    /// backend error when its ID/default format cannot be queried.
    pub fn default_device_probe(&self) -> Result<CpalDeviceProbe, OutputError> {
        let device = self
            .host
            .default_output_device()
            .ok_or(OutputError::DeviceUnavailable)?;
        let default_id = device
            .id()
            .map_err(|error| map_cpal_error(&error))?
            .to_string();
        probe_device(&device, Some(&default_id)).map_err(|error| map_cpal_error(&error))
    }

    fn default_device_id(&self) -> Option<String> {
        self.host
            .default_output_device()
            .and_then(|device| device.id().ok())
            .map(|id| id.to_string())
    }

    fn selected_device(
        &self,
        requested: Option<&OutputDeviceId>,
    ) -> Result<cpal::Device, OutputError> {
        let Some(requested) = requested else {
            return self
                .host
                .default_output_device()
                .ok_or(OutputError::DeviceUnavailable);
        };
        let id = cpal::DeviceId::from_str(requested.as_str())
            .map_err(|_| OutputError::DeviceUnavailable)?;
        self.host
            .device_by_id(&id)
            .ok_or(OutputError::DeviceUnavailable)
    }
}

impl Default for CpalOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioOutput for CpalOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        let default_id = self.default_device_id();
        self.host
            .output_devices()
            .map_err(|error| map_cpal_error(&error))?
            .map(|device| {
                device_info(&device, default_id.as_deref()).map_err(|error| map_cpal_error(&error))
            })
            .collect()
    }

    fn start(
        &mut self,
        device: Option<&OutputDeviceId>,
        config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        if self.stream.is_some() {
            return Err(OutputError::AlreadyRunning);
        }
        config.validate().map_err(OutputError::InvalidConfig)?;
        let maximum_samples = config
            .interleaved_samples()
            .map_err(OutputError::InvalidConfig)?;
        let device = self.selected_device(device)?;
        let stream_config = cpal::StreamConfig {
            channels: config.channels,
            sample_rate: config.sample_rate_hz,
            buffer_size: config
                .requested_callback_frames
                .map_or(cpal::BufferSize::Default, cpal::BufferSize::Fixed),
        };
        let status = Arc::new(CallbackStatus::default());
        let stream = match config.sample_format {
            SampleFormat::Float32 => {
                build_float32_stream(&device, stream_config, maximum_samples, source, &status)
            }
            SampleFormat::Signed16 => {
                build_signed16_stream(&device, stream_config, maximum_samples, source, &status)
            }
        }
        .map_err(|error| map_cpal_error(&error))?;
        stream.play().map_err(|error| map_cpal_error(&error))?;
        self.status = status;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) {
        self.stream.take();
        self.status = Arc::new(CallbackStatus::default());
    }

    fn health(&self) -> OutputHealth {
        let status = self.status();
        if status.stream_errors > 0 {
            OutputHealth::StreamError
        } else if status.oversized_callbacks > 0 {
            OutputHealth::CallbackCapacityExceeded
        } else {
            OutputHealth::Healthy
        }
    }
}

fn map_cpal_error(error: &cpal::Error) -> OutputError {
    match error.kind() {
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::HostUnavailable => {
            OutputError::DeviceUnavailable
        }
        cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
            OutputError::UnsupportedFormat
        }
        _ => OutputError::Backend(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_clears_callback_status_from_the_released_stream() {
        let mut output = CpalOutput::new();
        output.status.callback(480);
        output.status.stream_error();

        output.stop();

        assert_eq!(output.status(), CpalOutputStatus::default());
        assert_eq!(output.health(), OutputHealth::Healthy);
    }
}
