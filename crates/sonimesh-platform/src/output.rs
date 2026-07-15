use core::fmt;

use crate::{OutputConfig, OutputConfigError, OutputDeviceId, OutputDeviceInfo};

/// Source invoked by the platform's realtime output callback.
///
/// Implementations must not allocate, lock, await, decode, log, enumerate
/// devices, or call UI IPC from [`Self::render`]. Underflow must write silence.
pub trait RenderSource: Send + 'static {
    /// Fills an interleaved output buffer completely.
    fn render(&mut self, interleaved_output: &mut [f32]);
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
}

/// Stable platform adapter error categories suitable for UI mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
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

impl std::error::Error for OutputError {}
