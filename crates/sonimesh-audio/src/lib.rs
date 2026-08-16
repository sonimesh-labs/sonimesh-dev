//! Bounded audio-domain primitives for SoniMesh.
//!
//! Types in this crate allocate no memory after construction. Platform output,
//! Opus bindings, and realtime source adapters remain separate boundaries.

mod arrival_jitter;
mod channel_map;
mod decode;
mod drift;
mod fixed_slot_mixer;
mod gain;
mod gain_control;
mod jitter;
mod jitter_target;
mod limiter;
mod mixer_membership;
mod opus_frame;
mod pcm_ring;
mod realtime_queue;
mod resampler;

pub use arrival_jitter::{ArrivalJitter, ArrivalJitterConfigError};
pub use channel_map::{ChannelMapper, ChannelMappingError, MAX_AUDIO_CHANNELS};
pub use decode::{DecodeStep, DecodeWorkerError, FrameDecoder, StreamDecodeWorker};
pub use drift::{
    DriftConfig, DriftConfigError, DriftController, DriftDecision, DriftResyncDirection,
    MAX_DRIFT_ADJUSTMENT_PPM, MAX_DRIFT_INTEGRAL_BLOCK_TICKS,
};
pub use fixed_slot_mixer::{
    DEFAULT_MIXER_SLOTS, FixedSlotMixer, MIXER_GAIN_RAMP_SAMPLES, MixerConfigError, MixerError,
};
pub use gain::GainSmoother;
pub use gain_control::{GainControlError, GainControlHandle, GainControlState, MAX_GAIN_LINEAR};
pub use jitter::{EncodedFrame, FrameError, InsertOutcome, JitterBuffer, PlayoutOutcome};
pub use jitter_target::{
    AdaptiveJitterTarget, JitterMode, JitterTargetConfig, JitterTargetConfigError,
    JitterTargetStep, MAX_JITTER_FRAME_DURATION_MS, MAX_JITTER_TARGET_FRAMES,
    MIN_JITTER_FRAME_DURATION_MS,
};
pub use limiter::{
    DEFAULT_CEILING, DEFAULT_RELEASE_COEFFICIENT, LimiterConfig, LimiterConfigError, LimiterError,
    PeakLimiter,
};
pub use mixer_membership::{
    CommandDrivenMixer, MAX_MIXER_COMMAND_QUEUE, MixerMembershipError, MixerMembershipHandle,
};
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
    LinearResampler, MAX_RESAMPLE_RATE_HZ, MIN_RESAMPLE_RATE_HZ, PolyphaseResampler,
    ResampleOutcome, ResamplerError,
};
