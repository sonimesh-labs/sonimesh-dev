//! Deterministic MLS test-signal generation, WAVE I/O, and correlation-based
//! latency offset estimation used for SoniMesh acoustic timing measurements.
//!
//! This crate is a standalone measurement utility. It generates a deterministic
//! reference signal, reads WAVE recordings, and estimates the sample offset
//! between a reference and a recording by coarse-to-fine cross-correlation. It
//! intentionally depends only on the standard library.

pub mod offset;
pub mod signal;
pub mod stats;
pub mod wav;

pub use offset::{OffsetConfig, OffsetError, OffsetEstimate, estimate_offset_ms};
pub use signal::{MLS_PERIOD, mls_sequence, test_signal};
pub use stats::{LatencySummary, percentile, summarize_ms};
pub use wav::{
    MonoWav, SampleFormat, WavError, WavInfo, encode_mono, encode_mono_16bit, inspect_mono,
    parse_channel, parse_mono, read_channel, read_info, read_mono, write_mono, write_mono_16bit,
};
