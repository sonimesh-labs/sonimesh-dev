use core::fmt;

/// Opaque platform output identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDeviceId(String);

impl OutputDeviceId {
    /// Creates an identifier returned by a platform enumerator.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the platform-specific value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Sample representation negotiated with the output device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleFormat {
    /// 32-bit floating point PCM.
    Float32,
    /// Signed 16-bit integer PCM.
    Signed16,
}

/// Concrete device format accepted outside the realtime callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputConfig {
    /// Device sample rate, which may differ from 48 kHz.
    pub sample_rate_hz: u32,
    /// Interleaved output channel count.
    pub channels: u16,
    /// Device sample representation.
    pub sample_format: SampleFormat,
    /// Requested callback size when the backend supports it.
    pub callback_frames: u32,
}

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
    fn devices(&self) -> Result<Vec<OutputDeviceId>, OutputError>;

    /// Opens and starts a stream. Stream recreation happens in this call, never
    /// in the output callback.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError`] for missing devices, unsupported formats, host
    /// errors, or an already-running stream.
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
            Self::DeviceUnavailable => formatter.write_str("output device is unavailable"),
            Self::UnsupportedFormat => formatter.write_str("output format is unsupported"),
            Self::AlreadyRunning => formatter.write_str("output stream is already running"),
            Self::Backend(message) => write!(formatter, "output backend failed: {message}"),
        }
    }
}

impl std::error::Error for OutputError {}
