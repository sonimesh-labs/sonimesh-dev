use core::fmt;

use crate::{OutputConfig, OutputConfigError, OutputDeviceId, OutputDeviceInfo};

/// Source invoked by the platform's realtime output callback.
///
/// Implementations must not allocate, lock, await, decode, log, enumerate
/// devices, or call UI IPC from [`Self::render`]. Underflow must write silence,
/// and sources should write finite normalized PCM samples. Windows output paths
/// initialize callback buffers to silence, then defensively silence non-finite
/// values and clamp amplitudes to the normalized range before handing data to
/// the platform.
pub trait RenderSource: Send + 'static {
    /// Fills an interleaved output buffer completely.
    fn render(&mut self, interleaved_output: &mut [f32]);
}

/// Creates a fresh realtime render source from the control domain.
///
/// Implementations may allocate or replace queue ownership here because this
/// method is only used for stream start and recreation, never from a callback.
pub trait RenderSourceFactory {
    /// Creates the source consumed by the next output stream.
    ///
    /// The selected output configuration is provided so a factory can install
    /// a bounded channel mapper or other format adapter before stream start.
    ///
    /// # Errors
    ///
    /// Returns [`RenderSourceFactoryError`] before the platform stream starts
    /// when source ownership, format adaptation, or preallocation fails.
    fn create(
        &mut self,
        config: &OutputConfig,
    ) -> Result<Box<dyn RenderSource>, RenderSourceFactoryError>;

    /// Records that the platform accepted a source created for `config`.
    ///
    /// This runs in the control domain after [`AudioOutput::start`] succeeds.
    /// Factories that publish low-rate metadata tied to a platform output
    /// lifetime can override it; ordinary factories need no lifecycle hook.
    fn output_started(&mut self, _config: &OutputConfig) {}

    /// Records that the platform stopped its current source.
    ///
    /// This runs in the control domain after [`AudioOutput::stop`] releases a
    /// source previously accepted by [`AudioOutput::start`]. Factories can clear
    /// metadata that is valid only while a callback source is active.
    fn output_stopped(&mut self) {}
}

/// Stable source-construction failure suitable for output lifecycle handling.
#[derive(Clone, Eq, PartialEq)]
pub enum RenderSourceFactoryError {
    /// The selected output format violates a platform-neutral bound.
    InvalidConfig(OutputConfigError),
    /// Source setup failed without a more stable platform-neutral category.
    Build(String),
}

impl fmt::Debug for RenderSourceFactoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => {
                formatter.debug_tuple("InvalidConfig").field(error).finish()
            }
            Self::Build(message) => formatter
                .debug_struct("Build")
                .field("message_len", &message.len())
                .finish(),
        }
    }
}

impl fmt::Display for RenderSourceFactoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => write!(formatter, "{error}"),
            Self::Build(message) => write!(formatter, "render source setup failed: {message}"),
        }
    }
}

impl std::error::Error for RenderSourceFactoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(error) => Some(error),
            Self::Build(_) => None,
        }
    }
}

/// Low-rate health state published by a platform output adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputHealth {
    /// No callback or backend fault has been observed.
    Healthy,
    /// Callback exceeded the preallocated safe output bound.
    CallbackCapacityExceeded,
    /// Backend reported an asynchronous stream error.
    StreamError,
}

/// Platform-owned output stream lifecycle.
pub trait AudioOutput {
    /// Lists output devices outside the realtime domain.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] when the host cannot enumerate devices.
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError>;

    /// Opens and starts a stream. Stream recreation happens in this call, never
    /// in the output callback.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] for invalid configuration, missing devices,
    /// unsupported formats, host errors, or an already-running stream.
    fn start(
        &mut self,
        device: Option<&OutputDeviceId>,
        config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError>;

    /// Stops and releases the current stream outside the realtime domain.
    fn stop(&mut self);

    /// Returns the latest low-rate health state without touching the callback.
    #[must_use]
    fn health(&self) -> OutputHealth {
        OutputHealth::Healthy
    }
}

/// Stable platform adapter error categories suitable for UI mapping.
#[derive(Clone, Eq, PartialEq)]
pub enum OutputError {
    /// Configuration exceeds a platform-neutral hard bound.
    InvalidConfig(OutputConfigError),
    /// Requested or default output device is unavailable.
    DeviceUnavailable,
    /// Device cannot render the requested format.
    UnsupportedFormat,
    /// Output is already active.
    AlreadyRunning,
    /// Backend failed without a more stable category.
    Backend(String),
}

impl fmt::Debug for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => {
                formatter.debug_tuple("InvalidConfig").field(error).finish()
            }
            Self::DeviceUnavailable => formatter.write_str("DeviceUnavailable"),
            Self::UnsupportedFormat => formatter.write_str("UnsupportedFormat"),
            Self::AlreadyRunning => formatter.write_str("AlreadyRunning"),
            Self::Backend(message) => formatter
                .debug_struct("Backend")
                .field("message_len", &message.len())
                .finish(),
        }
    }
}

impl fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(error) => write!(formatter, "{error}"),
            Self::DeviceUnavailable => formatter.write_str("output device is unavailable"),
            Self::UnsupportedFormat => formatter.write_str("output format is unsupported"),
            Self::AlreadyRunning => formatter.write_str("output stream is already running"),
            Self::Backend(message) => write!(formatter, "output backend failed: {message}"),
        }
    }
}

impl std::error::Error for OutputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(error) => Some(error),
            Self::DeviceUnavailable
            | Self::UnsupportedFormat
            | Self::AlreadyRunning
            | Self::Backend(_) => None,
        }
    }
}

#[cfg(test)]
mod debug_tests {
    use super::{OutputConfigError, OutputError, RenderSourceFactoryError};

    #[test]
    fn backend_debug_redacts_free_form_error_messages() {
        let factory_error = RenderSourceFactoryError::Build("Private Headphones".to_owned());
        let output_error = OutputError::Backend("wasapi:{persistent-device-id}".to_owned());

        let factory_debug = format!("{factory_error:?}");
        let output_debug = format!("{output_error:?}");

        assert!(factory_debug.contains("Build { message_len:"));
        assert!(output_debug.contains("Backend { message_len:"));
        assert!(!factory_debug.contains("Private Headphones"));
        assert!(!output_debug.contains("persistent-device-id"));
    }

    #[test]
    fn output_error_display_has_stable_messages() {
        assert_eq!(
            OutputError::DeviceUnavailable.to_string(),
            "output device is unavailable"
        );
        assert_eq!(
            OutputError::UnsupportedFormat.to_string(),
            "output format is unsupported"
        );
        assert_eq!(
            OutputError::AlreadyRunning.to_string(),
            "output stream is already running"
        );
        assert_eq!(
            OutputError::Backend("boom".to_owned()).to_string(),
            "output backend failed: boom"
        );
        assert_eq!(
            OutputError::InvalidConfig(OutputConfigError::InvalidSampleRate).to_string(),
            "output sample rate is outside the supported 8-192 kHz range"
        );
    }

    #[test]
    fn output_error_debug_never_exposes_free_form_payload() {
        for error in [
            OutputError::DeviceUnavailable,
            OutputError::UnsupportedFormat,
            OutputError::AlreadyRunning,
        ] {
            let debug = format!("{error:?}");
            assert!(
                !debug.contains('(') && !debug.contains('{'),
                "unexpected payload in debug output: {debug}"
            );
        }
        assert_eq!(
            format!("{:?}", OutputError::Backend("secret".to_owned())),
            "Backend { message_len: 6 }"
        );
    }

    #[test]
    fn output_error_source_links_config_errors() {
        let error = OutputError::InvalidConfig(OutputConfigError::InvalidChannelCount);
        let source = std::error::Error::source(&error);
        assert_eq!(
            source.map(ToString::to_string),
            Some(OutputConfigError::InvalidChannelCount.to_string())
        );
    }
}
