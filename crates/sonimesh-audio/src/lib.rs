//! Bounded audio-domain primitives for SoniMesh.
//!
//! Types in this crate allocate no memory after construction. Platform output,
//! Opus bindings, and realtime source adapters remain separate boundaries.

mod channel_map;
mod decode;
mod drift;
mod gain;
mod jitter;
mod opus_frame;
mod pcm_ring;
mod realtime_queue;
mod resampler;

pub use channel_map::{ChannelMapper, ChannelMappingError, MAX_AUDIO_CHANNELS};
pub use decode::{DecodeStep, DecodeWorkerError, FrameDecoder, StreamDecodeWorker};
pub use drift::{
    DriftConfig, DriftConfigError, DriftController, DriftDecision, DriftResyncDirection,
    MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS,
};
pub use gain::GainSmoother;
pub use jitter::{EncodedFrame, FrameError, InsertOutcome, JitterBuffer, PlayoutOutcome};
pub use opus_frame::{
    MAX_OPUS_FRAME_DURATION_MS, MIN_OPUS_FRAME_DURATION_MS, OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ,
    OpusFrameConfig, OpusFrameConfigError,
};
pub use pcm_ring::{PcmBlockRing, PcmPopOutcome, PcmPushOutcome};
pub use realtime_queue::{
    MAX_PCM_QUEUE_BLOCKS, RealtimePcmConsumer, RealtimePcmPopOutcome, RealtimePcmProducer,
    RealtimePcmPushOutcome, RealtimeQueueConfigError, realtime_pcm_queue,
};
pub use resampler::{
    LinearResampler, MAX_RESAMPLE_RATE_HZ, MIN_RESAMPLE_RATE_HZ, ResampleOutcome, ResamplerError,
};
