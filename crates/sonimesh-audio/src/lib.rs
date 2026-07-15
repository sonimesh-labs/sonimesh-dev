//! Bounded audio-domain primitives for SoniMesh.
//!
//! Types in this crate allocate no memory after construction. Platform output,
//! Opus bindings, and nominal sample-rate conversion remain separate adapters.

mod gain;
mod jitter;
mod pcm_ring;
mod realtime_queue;

pub use gain::GainSmoother;
pub use jitter::{EncodedFrame, FrameError, InsertOutcome, JitterBuffer, PlayoutOutcome};
pub use pcm_ring::{PcmBlockRing, PcmPopOutcome, PcmPushOutcome};
pub use realtime_queue::{
    MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer,
    RealtimePcmPushOutcome, RealtimeQueueConfigError, realtime_pcm_queue,
};
