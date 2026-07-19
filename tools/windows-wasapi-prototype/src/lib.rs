//! Direct Windows WASAPI feasibility adapter kept outside stable platform code.
//!
//! This crate exists solely to compare direct shared-mode WASAPI behaviour with
//! the CPAL feasibility adapter. It is not a selected product backend.

#[cfg(windows)]
mod status;
#[cfg(windows)]
mod wasapi;

#[cfg(windows)]
pub use status::WasapiOutputStatus;
#[cfg(windows)]
pub use wasapi::WasapiOutput;
