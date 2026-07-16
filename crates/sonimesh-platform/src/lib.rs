//! Platform adapter contracts kept independent of Tauri and web views.

mod config;
#[cfg(all(windows, feature = "cpal-output"))]
mod cpal_callback;
#[cfg(all(windows, feature = "cpal-output"))]
mod cpal_output;
#[cfg(all(windows, feature = "cpal-output"))]
mod cpal_probe;
#[cfg(all(windows, feature = "cpal-output"))]
mod cpal_status;
mod device;
mod output;

pub use config::{
    MAX_CALLBACK_FRAMES, MAX_OUTPUT_CHANNELS, MAX_OUTPUT_SAMPLE_RATE_HZ, MIN_CALLBACK_FRAMES,
    MIN_OUTPUT_SAMPLE_RATE_HZ, OutputConfig, OutputConfigError, SampleFormat,
};
#[cfg(all(windows, feature = "cpal-output"))]
pub use cpal_output::CpalOutput;
#[cfg(all(windows, feature = "cpal-output"))]
pub use cpal_probe::{CpalBufferSize, CpalDeviceProbe};
#[cfg(all(windows, feature = "cpal-output"))]
pub use cpal_status::CpalOutputStatus;
pub use device::{OutputDeviceId, OutputDeviceInfo};
pub use output::{AudioOutput, OutputError, OutputHealth, RenderSource, RenderSourceFactory};
