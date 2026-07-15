//! Platform adapter contracts kept independent of Tauri and web views.

mod config;
mod device;
mod output;

pub use config::{
    MAX_CALLBACK_FRAMES, MAX_OUTPUT_CHANNELS, MAX_OUTPUT_SAMPLE_RATE_HZ, MIN_CALLBACK_FRAMES,
    MIN_OUTPUT_SAMPLE_RATE_HZ, OutputConfig, OutputConfigError, SampleFormat,
};
pub use device::{OutputDeviceId, OutputDeviceInfo};
pub use output::{AudioOutput, OutputError, RenderSource};
