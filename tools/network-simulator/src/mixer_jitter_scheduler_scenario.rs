//! Deterministic target-gated playout feeding the fixed-slot mixer.
//!
//! This scenario ties ADR 0016 to ADR 0015: each of `STREAMS` source streams
//! traverses one shared [`VirtualNetwork`], a per-stream
//! [`AdaptiveJitterTarget`] gates that stream's playout (waiting while fewer
//! than the target encoded frames are buffered, draining without gating once
//! no frames are in flight, forcing a playout when the gate deadline expires),
//! and every played-out frame is decoded to a deterministic finite PCM block
//! and pushed into that stream's slot in a fixed-slot [`CommandDrivenMixer`].
//! The callback renders the bus every tick, so the report proves the Phase 2
//! receiver chain — adaptive target, gated playout, and fixed-slot mixing —
//! stays bounded and finite over one lossy shared link while a gated or
//! starved slot never blocks the other slots. It makes no physical latency
//! claim.

use std::fmt;
use std::sync::Arc;

use sonimesh_audio::{
    AdaptiveJitterTarget, CommandDrivenMixer, EncodedFrame, GainControlHandle, InsertOutcome,
    JitterBuffer, JitterMode, JitterTargetConfig, JitterTargetConfigError, JitterTargetStep,
    OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutOutcome, RealtimePcmProducer,
    RealtimeQueueConfigError, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::multi_stream_scenario::{MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS};
use crate::scenario::SimulationReport;
use crate::{ClockError, NetworkProfile, NetworkStats, ProfileError, VirtualClock, VirtualNetwork};

/// Hard bound on sender frames per stream.
const MAX_MIXER_JITTER_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of every stream's jitter buffer and PCM queue.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Hard bound on consecutive gated playout ticks before a forced playout.
const MAX_GATE_TICKS_BOUND: u32 = 64;
/// Default fixed slot count of the mixer membership table.
pub const DEFAULT_MIXER_JITTER_SLOTS: usize = 8;
/// Default output-format PCM block size (20 ms at 48 kHz stereo).
pub const DEFAULT_MIXER_JITTER_PCM_BLOCK: usize = 960;

/// One deterministic combined jitter-scheduler and mixer configuration.
///
/// `STREAMS` source streams share one link and profile; each stream owns an
/// adaptive target, a target-gated scheduler, and one slot of a fixed
/// `SLOTS`-slot [`CommandDrivenMixer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerJitterSchedulerScenarioConfig<const STREAMS: usize> {
    /// Number of sender frames per stream.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and scheduler.
    pub frame_duration_ms: u64,
    /// Time of the first scheduler tick for every stream.
    pub initial_playout_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Shared virtual link impairments applied to every stream.
    pub network: NetworkProfile,
    /// Bounded adaptive-target configuration; its frame duration and buffer
    /// capacity must match the scheduling frame duration and jitter capacity.
    pub target: JitterTargetConfig,
    /// Consecutive gated ticks allowed before the scheduler plays out anyway.
    pub max_gate_ticks: u32,
}

impl<const STREAMS: usize> Default for MixerJitterSchedulerScenarioConfig<STREAMS> {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            initial_playout_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_BUFFER_CAPACITY),
            max_gate_ticks: 8,
        }
    }
}

/// Deterministic per-stream target-gated scheduler counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamJitterSchedulerReport {
    /// Per-playout transport counters; the `network` field is zero.
    pub playout: SimulationReport,
    /// Adaptive target water level at the end of the simulation, in frames.
    pub final_target_frames: usize,
    /// Minimum water level the controller can reach, in frames.
    pub min_frames: usize,
    /// Mode base water level, in frames.
    pub base_frames: usize,
    /// Maximum water level the controller can reach, in frames.
    pub max_frames: usize,
    /// Smoothed packet-arrival-jitter estimate at the end, in milliseconds.
    pub final_smoothed_jitter_ms: u32,
    /// Total target-raising steps taken.
    pub raised_steps: u32,
    /// Total target-lowering steps taken.
    pub lowered_steps: u32,
    /// Controller observations, one per scheduler tick.
    pub observations: u32,
    /// Scheduler ticks that waited for the target instead of playing out.
    pub gated_ticks: u32,
    /// Playouts executed below the target after the gate deadline expired.
    pub forced_playouts: u32,
    /// Encoded frames buffered when the simulation ended.
    pub final_buffered_frames: usize,
}

/// Deterministic mixer-bus counters for one combined run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerJitterMixerReport<const SLOTS: usize> {
    /// Fixed slot count of the mixer table.
    pub slots: usize,
    /// Streams the callback renders after the final drain.
    pub active_streams: usize,
    /// Output callbacks rendered.
    pub renders: u32,
    /// Per-slot PCM underflow events recorded by the callback.
    pub slot_underflows: [u64; SLOTS],
    /// Per-slot PCM refill-limit events recorded by the callback.
    pub slot_refill_limit_hits: [u64; SLOTS],
    /// Per-slot discontinuity events recorded by the callback.
    pub slot_discontinuities: [u64; SLOTS],
    /// True when every rendered sample stayed finite.
    pub output_finite: bool,
}

/// One deterministic combined jitter-scheduler and mixer report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerJitterSchedulerSimulationReport<const STREAMS: usize, const SLOTS: usize> {
    /// Shared virtual link counters.
    pub network: NetworkStats,
    /// Per-stream adaptive-target and scheduler counters.
    pub streams: [StreamJitterSchedulerReport; STREAMS],
    /// Mixer-bus counters.
    pub mixer: MixerJitterMixerReport<SLOTS>,
}

/// Invalid simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerJitterSchedulerScenarioError {
    /// Stream count must be between the shared multi-stream bound.
    InvalidStreamCount,
    /// Slot count must be at least the stream count and non-zero.
    InvalidSlotCount,
    /// Frame count must be between one and the scenario hard bound.
    InvalidFrameCount,
    /// Frame duration must be one negotiated version-one value: 10 ms or 20 ms.
    InvalidFrameDuration,
    /// Initial scheduler tick must be no more than one second.
    InvalidInitialPlayout,
    /// Gate deadline must be between one and the explicit hard bound.
    InvalidMaxGateTicks,
    /// Complete media datagram size must fit the version-one packet limit.
    InvalidMediaDatagramBytes,
    /// Controller frame duration must match the scheduling frame duration.
    TargetFrameDurationMismatch,
    /// Controller buffer capacity must match the jitter buffer capacity.
    TargetBufferCapacityMismatch,
    /// Adaptive target configuration is invalid.
    Target(JitterTargetConfigError),
    /// Network profile is invalid.
    Profile(ProfileError),
    /// Virtual time overflowed.
    Clock(ClockError),
    /// A bounded mixer command queue rejected a membership change.
    CommandQueueFull,
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
    /// The hard deadline elapsed before every stream consumed every frame.
    SimulationDidNotFinish,
}

impl fmt::Display for MixerJitterSchedulerScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter.write_str(
                "invalid mixer jitter scenario: stream count is outside the explicit bound",
            ),
            Self::InvalidSlotCount => formatter.write_str(
                "invalid mixer jitter scenario: slot count is below the stream count",
            ),
            Self::InvalidFrameCount => formatter.write_str(
                "invalid mixer jitter scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid mixer jitter scenario: frame duration is not a negotiated 10 ms or 20 ms value",
            ),
            Self::InvalidInitialPlayout => formatter.write_str(
                "invalid mixer jitter scenario: initial scheduler tick is above the one-second bound",
            ),
            Self::InvalidMaxGateTicks => formatter.write_str(
                "invalid mixer jitter scenario: gate deadline is outside the explicit bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid mixer jitter scenario: media datagram size is outside the version-one bound",
            ),
            Self::TargetFrameDurationMismatch => formatter.write_str(
                "invalid mixer jitter scenario: target frame duration differs from the scheduling frame duration",
            ),
            Self::TargetBufferCapacityMismatch => formatter.write_str(
                "invalid mixer jitter scenario: target buffer capacity differs from the jitter buffer capacity",
            ),
            Self::Target(error) => write!(formatter, "invalid mixer jitter scenario: {error}"),
            Self::Profile(error) => write!(formatter, "invalid mixer jitter scenario: {error}"),
            Self::Clock(error) => write!(formatter, "invalid mixer jitter scenario: {error}"),
            Self::CommandQueueFull => formatter.write_str(
                "invalid mixer jitter scenario: the bounded membership command queue rejected a change",
            ),
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid mixer jitter scenario: a compile-time capacity contradicted its runtime use",
            ),
            Self::SimulationDidNotFinish => formatter.write_str(
                "invalid mixer jitter scenario: the hard deadline elapsed before every frame was consumed",
            ),
        }
    }
}

impl std::error::Error for MixerJitterSchedulerScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            _ => None,
        }
    }
}

/// Deterministic finite samples each stream contributes when it renders.
#[allow(clippy::cast_precision_loss)]
fn finite_block<const N: usize>(stream: usize) -> [f32; N] {
    [1.0 + 0.125 * (stream as f32); N]
}

fn sample_timestamp(sequence: u32, frame: OpusFrameConfig) -> u64 {
    u64::from(sequence).saturating_mul(frame.samples_per_channel() as u64)
}

/// Mutable per-stream scheduler bookkeeping shared by one playout tick.
#[derive(Clone, Copy, Default)]
struct SchedulerState {
    consumed_frames: u32,
    gated_in_a_row: u32,
}

/// Runs one deterministic combined jitter-scheduler and mixer simulation.
///
/// Each stream's adaptive target gates playout; every played-out frame is
/// decoded to a finite PCM block and pushed into that stream's mixer slot, and
/// the callback renders the bus every tick until every stream consumed every
/// frame or the hard deadline elapses.
///
/// # Errors
///
/// Returns a configuration or virtual-time error when the scenario cannot be
/// run deterministically, or [`MixerJitterSchedulerScenarioError::SimulationDidNotFinish`]
/// when the hard deadline elapses before every stream consumed every frame.
#[allow(clippy::too_many_lines)]
pub fn simulate_mixer_jitter_scheduler<
    const STREAMS: usize,
    const SLOTS: usize,
    const PCM_SAMPLES_PER_BLOCK: usize,
>(
    config: MixerJitterSchedulerScenarioConfig<STREAMS>,
) -> Result<MixerJitterSchedulerSimulationReport<STREAMS, SLOTS>, MixerJitterSchedulerScenarioError>
{
    let (frame, targets) = validate::<STREAMS, SLOTS>(config)?;
    let mut targets = targets;
    let mut network = VirtualNetwork::<(u8, u32)>::new(config.network)
        .map_err(MixerJitterSchedulerScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitters: [JitterBuffer<JITTER_BUFFER_CAPACITY, 8>; STREAMS] =
        std::array::from_fn(|_| JitterBuffer::<JITTER_BUFFER_CAPACITY, 8>::with_start_sequence(0));
    let mut streams = std::array::from_fn(|_| StreamJitterSchedulerReport {
        playout: SimulationReport::default(),
        final_target_frames: 0,
        min_frames: 0,
        base_frames: 0,
        max_frames: 0,
        final_smoothed_jitter_ms: 0,
        raised_steps: 0,
        lowered_steps: 0,
        observations: 0,
        gated_ticks: 0,
        forced_playouts: 0,
        final_buffered_frames: 0,
    });
    for stream in 0..STREAMS {
        streams[stream].min_frames = targets[stream].min_frames();
        streams[stream].base_frames = targets[stream].base_frames();
        streams[stream].max_frames = targets[stream].max_frames();
    }
    let mut states = [SchedulerState::default(); STREAMS];
    let mut last_arrival_jitter_ms = [0_u32; STREAMS];
    let mut next_send_sequence = [0_u32; STREAMS];
    let mut next_send_ms = [0_u64; STREAMS];
    let mut next_playout_ms = [config.initial_playout_ms; STREAMS];

    let mut mixer = CommandDrivenMixer::<SLOTS, PCM_SAMPLES_PER_BLOCK>::new()
        .map_err(|_| MixerJitterSchedulerScenarioError::InternalCapacityInvariant)?;
    let membership = mixer.membership();
    let mut producers: [Option<RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>>; STREAMS] =
        std::array::from_fn(|_| None);
    let mut slot_counters: [Option<Arc<StreamCounters>>; STREAMS] = std::array::from_fn(|_| None);
    for stream in 0..STREAMS {
        let counters = Arc::new(StreamCounters::default());
        let (producer, consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(
            JITTER_BUFFER_CAPACITY,
        )
        .map_err(|error| match error {
            RealtimeQueueConfigError::ZeroCapacity
            | RealtimeQueueConfigError::CapacityTooLarge
            | RealtimeQueueConfigError::ZeroSamplesPerBlock => {
                MixerJitterSchedulerScenarioError::InternalCapacityInvariant
            }
        })?;
        membership
            .insert(
                consumer,
                GainControlHandle::new(),
                Some(Arc::clone(&counters)),
            )
            .map_err(|_| MixerJitterSchedulerScenarioError::CommandQueueFull)?;
        producers[stream] = Some(producer);
        slot_counters[stream] = Some(counters);
    }

    let nominal_completion_ms = config.initial_playout_ms.saturating_add(
        u64::from(config.frames.saturating_sub(1)).saturating_mul(config.frame_duration_ms),
    );
    let hard_end_ms = nominal_completion_ms
        .saturating_add(
            u64::from(config.frames).saturating_mul(
                u64::from(config.max_gate_ticks.saturating_add(2))
                    .saturating_mul(config.frame_duration_ms),
            ),
        )
        .saturating_add(config.network.base_delay_ms)
        .saturating_add(config.network.jitter_ms)
        .saturating_add(config.network.reorder_delay_ms)
        .saturating_add(2);

    let mut renders = 0_u32;
    let mut output_finite = true;
    let all_consumed = |states: &[SchedulerState; STREAMS]| {
        states
            .iter()
            .all(|state| state.consumed_frames == config.frames)
    };
    while !all_consumed(&states) && clock.now_ms() <= hard_end_ms {
        let now_ms = clock.now_ms();
        for stream in 0..STREAMS {
            if next_send_sequence[stream] < config.frames && now_ms == next_send_ms[stream] {
                let tag = u8::try_from(stream)
                    .map_err(|_| MixerJitterSchedulerScenarioError::InternalCapacityInvariant)?;
                network.send(
                    now_ms,
                    (tag, next_send_sequence[stream]),
                    u64::from(config.media_datagram_bytes),
                );
                next_send_sequence[stream] += 1;
                next_send_ms[stream] =
                    next_send_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }

        for delivery in network.drain_ready(now_ms) {
            let (tag, sequence) = delivery.payload;
            let stream = usize::from(tag);
            let delay_ms = delivery.delivered_at_ms.saturating_sub(delivery.sent_at_ms);
            last_arrival_jitter_ms[stream] =
                u32::try_from(delay_ms.abs_diff(config.network.base_delay_ms)).unwrap_or(u32::MAX);
            let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| MixerJitterSchedulerScenarioError::InternalCapacityInvariant)?;
            match jitters[stream].insert(encoded) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => streams[stream].playout.reordered_arrivals += 1,
                InsertOutcome::Duplicate => streams[stream].playout.duplicate_frames += 1,
                InsertOutcome::Late => streams[stream].playout.late_frames += 1,
                InsertOutcome::Resynchronized { .. } => {
                    streams[stream].playout.resynchronizations += 1;
                }
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(MixerJitterSchedulerScenarioError::InternalCapacityInvariant);
                }
            }
        }

        for stream in 0..STREAMS {
            if states[stream].consumed_frames < config.frames && now_ms == next_playout_ms[stream] {
                let arrivals_pending = next_send_sequence[stream] < config.frames
                    || network.stats().scheduled_copies > network.stats().delivered_copies;
                apply_playout_tick::<PCM_SAMPLES_PER_BLOCK>(
                    stream,
                    &mut streams[stream],
                    &mut targets[stream],
                    &mut jitters[stream],
                    last_arrival_jitter_ms[stream],
                    arrivals_pending,
                    config.max_gate_ticks,
                    &mut states[stream],
                    producers[stream].as_mut(),
                );
                next_playout_ms[stream] =
                    next_playout_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }

        let mut output = [0.0f32; PCM_SAMPLES_PER_BLOCK];
        mixer.render(&mut output);
        renders += 1;
        if !output.iter().all(|sample| sample.is_finite()) {
            output_finite = false;
        }
        clock
            .advance_by(1)
            .map_err(MixerJitterSchedulerScenarioError::Clock)?;
    }

    if !all_consumed(&states) {
        return Err(MixerJitterSchedulerScenarioError::SimulationDidNotFinish);
    }
    let mut slot_underflows = [0_u64; SLOTS];
    let mut slot_refill_limit_hits = [0_u64; SLOTS];
    let mut slot_discontinuities = [0_u64; SLOTS];
    for stream in 0..STREAMS {
        streams[stream].final_target_frames = targets[stream].target_frames();
        streams[stream].final_smoothed_jitter_ms = targets[stream].smoothed_jitter_ms();
        streams[stream].final_buffered_frames = jitters[stream].len();
        if let Some(counters) = slot_counters[stream].as_ref() {
            let snapshot = counters.snapshot();
            slot_underflows[stream] = snapshot.pcm_underflows;
            slot_refill_limit_hits[stream] = snapshot.pcm_refill_limit_hits;
            slot_discontinuities[stream] = snapshot.pcm_discontinuities;
        }
    }

    Ok(MixerJitterSchedulerSimulationReport {
        network: network.stats(),
        streams,
        mixer: MixerJitterMixerReport {
            slots: SLOTS,
            active_streams: mixer.active(),
            renders,
            slot_underflows,
            slot_refill_limit_hits,
            slot_discontinuities,
            output_finite,
        },
    })
}

/// Applies one target-gated scheduler tick, feeds the adaptive target, and
/// pushes any consumed frame into the stream's mixer slot.
#[allow(clippy::too_many_arguments)]
fn apply_playout_tick<const PCM_SAMPLES_PER_BLOCK: usize>(
    stream: usize,
    report: &mut StreamJitterSchedulerReport,
    target: &mut AdaptiveJitterTarget,
    jitter: &mut JitterBuffer<JITTER_BUFFER_CAPACITY, 8>,
    arrival_jitter_ms: u32,
    arrivals_pending: bool,
    max_gate_ticks: u32,
    state: &mut SchedulerState,
    producer: Option<&mut RealtimePcmProducer<PCM_SAMPLES_PER_BLOCK>>,
) {
    let buffered = jitter.len();
    let below_target = buffered < target.target_frames();
    let gate_expired = state.gated_in_a_row >= max_gate_ticks;
    let mut took = false;
    if !arrivals_pending || !below_target || gate_expired {
        match jitter.take_next() {
            PlayoutOutcome::Frame(_) => {
                report.playout.playout_frames += 1;
                if let Some(producer) = producer {
                    let _ = producer.push(&finite_block::<PCM_SAMPLES_PER_BLOCK>(stream));
                }
            }
            PlayoutOutcome::Missing { .. } => report.playout.plc_frames += 1,
            PlayoutOutcome::NotReady => {}
        }
        took = report.playout.playout_frames + report.playout.plc_frames > state.consumed_frames;
    }
    if took {
        state.consumed_frames += 1;
        state.gated_in_a_row = 0;
        if below_target && arrivals_pending {
            report.forced_playouts += 1;
        }
    } else {
        report.gated_ticks += 1;
        state.gated_in_a_row += 1;
    }
    report.observations += 1;
    match target.observe(arrival_jitter_ms, buffered) {
        JitterTargetStep::Raised => report.raised_steps += 1,
        JitterTargetStep::Lowered => report.lowered_steps += 1,
        JitterTargetStep::Held => {}
    }
}

fn validate<const STREAMS: usize, const SLOTS: usize>(
    config: MixerJitterSchedulerScenarioConfig<STREAMS>,
) -> Result<(OpusFrameConfig, [AdaptiveJitterTarget; STREAMS]), MixerJitterSchedulerScenarioError> {
    if !(MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS).contains(&STREAMS) {
        return Err(MixerJitterSchedulerScenarioError::InvalidStreamCount);
    }
    if SLOTS == 0 || SLOTS < STREAMS {
        return Err(MixerJitterSchedulerScenarioError::InvalidSlotCount);
    }
    if config.frames == 0 || config.frames > MAX_MIXER_JITTER_FRAMES {
        return Err(MixerJitterSchedulerScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| MixerJitterSchedulerScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| MixerJitterSchedulerScenarioError::InvalidFrameDuration)?;
    if config.target.frame_duration_ms != u32::from(frame_duration_ms) {
        return Err(MixerJitterSchedulerScenarioError::TargetFrameDurationMismatch);
    }
    config
        .target
        .validate()
        .map_err(MixerJitterSchedulerScenarioError::Target)?;
    if config.target.buffer_capacity_frames != JITTER_BUFFER_CAPACITY {
        return Err(MixerJitterSchedulerScenarioError::TargetBufferCapacityMismatch);
    }
    if config.initial_playout_ms > 1_000 {
        return Err(MixerJitterSchedulerScenarioError::InvalidInitialPlayout);
    }
    if config.max_gate_ticks == 0 || config.max_gate_ticks > MAX_GATE_TICKS_BOUND {
        return Err(MixerJitterSchedulerScenarioError::InvalidMaxGateTicks);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(MixerJitterSchedulerScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(MixerJitterSchedulerScenarioError::Profile)?;
    let targets = std::array::from_fn(|_| {
        AdaptiveJitterTarget::new(config.target)
            .expect("validated target configuration must construct")
    });
    Ok((frame, targets))
}
#[cfg(test)]
mod tests {
    use std::error::Error;

    use sonimesh_audio::{JitterMode, JitterTargetConfig, JitterTargetConfigError};

    use crate::{ClockError, MAX_BANDWIDTH_BYTES_PER_SECOND, ProfileError};

    use super::*;

    const SLOTS: usize = 4;
    const PCM_BLOCK: usize = 64;

    fn clean_profile() -> NetworkProfile {
        let mut network = NetworkProfile::stable(0x0A0C_2026);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        network
    }

    #[test]
    fn stream_or_slot_count_outside_the_explicit_bound_is_rejected() {
        assert_eq!(
            simulate_mixer_jitter_scheduler::<0, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig::<0>::default()
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<1, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig::<1>::default()
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<5, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig::<5>::default()
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, 1, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig::<2>::default()
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidSlotCount)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, 0, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig::<2>::default()
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidSlotCount)
        );
    }

    #[test]
    fn clean_shared_link_consumes_every_frame_and_keeps_the_bus_finite() {
        let config = MixerJitterSchedulerScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            initial_playout_ms: 10,
            network: clean_profile(),
            target: JitterTargetConfig::preset(JitterMode::Balanced, 10, JITTER_BUFFER_CAPACITY),
            ..MixerJitterSchedulerScenarioConfig::default()
        };

        let first = simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(config).unwrap();
        let second = simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        assert_eq!(first, second, "the simulation must be deterministic");
        for stream in &first.streams {
            assert_eq!(stream.playout.playout_frames, config.frames);
            assert_eq!(stream.playout.plc_frames, 0);
            assert_eq!(stream.final_target_frames, stream.base_frames);
            assert!(stream.observations > 0);
        }
        assert_eq!(first.mixer.active_streams, 2);
        assert_eq!(first.mixer.slots, SLOTS);
        assert!(first.mixer.renders > 0);
        assert!(first.mixer.output_finite);
    }

    #[test]
    fn shared_link_loss_gates_playout_without_stalling_the_bus() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        // A burst must outlast the gate deadline (default `max_gate_ticks` is
        // 8): a 3/20 burst only drains ~3 ticks below the target, so the
        // deadline never expires and no forced playout is exercised. Dropping
        // 18 of every 20 packets starves each stream for ~9 ticks, which forces
        // playouts past the deadline while still finishing inside the hard end.
        network.burst_length_packets = 18;
        let config = MixerJitterSchedulerScenarioConfig {
            frames: 100,
            network,
            ..MixerJitterSchedulerScenarioConfig::default()
        };

        let report = simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(config).unwrap();

        for stream in &report.streams {
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames,
                "every frame must be consumed by playout or PLC"
            );
            assert!(
                (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
                "the adaptive target must stay within its explicit bounds"
            );
        }
        let total_plc: u32 = report
            .streams
            .iter()
            .map(|stream| stream.playout.plc_frames)
            .sum();
        assert!(
            total_plc > 0,
            "shared-link loss must reach the jitter buffers"
        );
        assert!(
            report
                .streams
                .iter()
                .any(|stream| stream.forced_playouts > 0),
            "loss below the target must exercise forced playouts"
        );
        assert!(
            report
                .mixer
                .slot_underflows
                .iter()
                .any(|&underflows| underflows > 0),
            "a gated or starved slot must record PCM underflows"
        );
        assert_eq!(report.mixer.active_streams, 2);
        assert!(report.mixer.output_finite);
    }

    #[test]
    fn rejects_invalid_scenario_configuration() {
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    frames: 0,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidFrameCount)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    frame_duration_ms: 15,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidFrameDuration)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    initial_playout_ms: 1_001,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidInitialPlayout)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    max_gate_ticks: 0,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidMaxGateTicks)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    max_gate_ticks: MAX_GATE_TICKS_BOUND + 1,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidMaxGateTicks)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    media_datagram_bytes: 0,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::InvalidMediaDatagramBytes)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    target: JitterTargetConfig::preset(
                        JitterMode::Balanced,
                        10,
                        JITTER_BUFFER_CAPACITY
                    ),
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::TargetFrameDurationMismatch)
        );
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 32),
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::TargetBufferCapacityMismatch)
        );
        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        assert_eq!(
            simulate_mixer_jitter_scheduler::<2, SLOTS, PCM_BLOCK>(
                MixerJitterSchedulerScenarioConfig {
                    network,
                    ..MixerJitterSchedulerScenarioConfig::default()
                }
            ),
            Err(MixerJitterSchedulerScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let target =
            MixerJitterSchedulerScenarioError::Target(JitterTargetConfigError::ZeroCapacity);
        assert!(
            Error::source(&target)
                .and_then(|source| source.downcast_ref::<JitterTargetConfigError>())
                .is_some()
        );

        let profile = MixerJitterSchedulerScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = MixerJitterSchedulerScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&MixerJitterSchedulerScenarioError::InvalidStreamCount).is_none());
        assert!(
            Error::source(&MixerJitterSchedulerScenarioError::InternalCapacityInvariant).is_none()
        );
    }
}
