//! Platform adapter contracts kept independent of Tauri and web views.

mod output;

pub use output::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputError, RenderSource, SampleFormat,
};
