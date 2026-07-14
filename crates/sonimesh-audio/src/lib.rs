//! Bounded audio-domain primitives for SoniMesh.
//!
//! Types in this crate allocate no memory after construction. Platform output,
//! Opus bindings, and nominal sample-rate conversion remain separate adapters.

mod gain;
mod jitter;
mod pcm_ring;

pub use gain::GainSmoother;
pub use jitter::{EncodedFrame, FrameError, InsertOutcome, JitterBuffer, PlayoutOutcome};
pub use pcm_ring::{PcmBlockRing, PcmPopOutcome, PcmPushOutcome};
