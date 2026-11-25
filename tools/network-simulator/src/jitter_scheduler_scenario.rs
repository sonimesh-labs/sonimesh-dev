//! Deterministic target-gated playout scheduler simulation.
//!
//! One source stream traverses a [`VirtualNetwork`] while a per-stream
//! [`AdaptiveJitterTarget`] (ADR 0016) gates playout: the scheduler waits
//! while fewer than the target encoded frames are buffered and plays out
//! once the target is reached, drains without gating once no further frames
//! are in flight, and forces a playout when the gate deadline expires so a
//! mid-stream stall can never become unbounded. The report carries the
//! bounded target trajectory and scheduler behavior (gated ticks, forced
//! playouts, added completion latency) as deterministic feasibility evidence
//! for the Phase 2 playout scheduler; it makes no physical latency claim.

use std::fmt;

use sonimesh_audio::{
    AdaptiveJitterTarget, EncodedFrame, InsertOutcome, JitterBuffer, JitterMode,
    JitterTargetConfig, JitterTargetConfigError, JitterTargetStep, MAX_GATE_TICKS, OPUS_CHANNELS,
    OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutDecision, PlayoutOutcome, TargetGatedPlayout,
    TargetGatedPlayoutConfig,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::{
    ClockError, Delivery, NetworkProfile, NetworkStats, ProfileError, SimulationReport,
    VirtualClock, VirtualNetwork,
};

/// Hard bound on sender frames in the scheduler scenario.
const MAX_SCHEDULER_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of the scenario's jitter buffer.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Hard bound on consecutive gated playout ticks before a forced playout.
const MAX_GATE_TICKS_BOUND: u32 = MAX_GATE_TICKS;

/// One deterministic target-gated scheduler simulation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterSchedulerScenarioConfig {
    /// Number of sender frames.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and scheduler.
    pub frame_duration_ms: u64,
    /// Time of the first scheduler tick.
    pub initial_playout_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Virtual network impairments that generate the arrival-jitter signal.
    pub network: NetworkProfile,
    /// Bounded controller configuration; its frame duration and buffer capacity
    /// must match the scheduling frame duration and jitter buffer capacity.
    pub target: JitterTargetConfig,
    /// Consecutive gated ticks allowed before the scheduler plays out anyway.
    pub max_gate_ticks: u32,
}

impl Default for JitterSchedulerScenarioConfig {
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

/// Deterministic target-gated scheduler counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterSchedulerSimulationReport {
    /// Shared virtual link counters.
    pub network: NetworkStats,
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
    /// Largest raw arrival-jitter deviation observed, in milliseconds.
    pub observed_jitter_max_ms: u32,
    /// Encoded frames buffered when the simulation ended.
    pub final_buffered_frames: usize,
    /// Scheduler ticks that waited for the target instead of playing out.
    pub gated_ticks: u32,
    /// Playouts executed below the target after the gate deadline expired.
    pub forced_playouts: u32,
    /// Wall-clock delay beyond the nominal playout completion time.
    pub excess_latency_ms: u64,
}

/// Invalid scheduler simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitterSchedulerScenarioError {
    /// Frame count must be between one and the scenario hard bound.
    InvalidFrameCount,
    /// Frame duration must be one negotiated version-one value: 10 ms or 20 ms.
    InvalidFrameDuration,
    /// Initial scheduler tick must be no more than one second.
    InvalidInitialPlayout,
    /// Gate deadline must be between one and the explicit hard bound.
    InvalidMaxGateTicks,
    /// Sender batch must contain between one and eight frames.
    InvalidSenderBatch,
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
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
    /// The hard deadline elapsed before every frame was consumed.
    SimulationDidNotFinish,
}

impl fmt::Display for JitterSchedulerScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameCount => formatter
                .write_str("invalid scheduler scenario: frame count is outside the explicit bound"),
            Self::InvalidFrameDuration => formatter
                .write_str("invalid scheduler scenario: frame duration is not a negotiated value"),
            Self::InvalidInitialPlayout => formatter
                .write_str("invalid scheduler scenario: initial scheduler tick exceeds one second"),
            Self::InvalidMaxGateTicks => formatter.write_str(
                "invalid scheduler scenario: gate deadline is outside the explicit bound",
            ),
            Self::InvalidSenderBatch => formatter.write_str(
                "invalid scheduler scenario: sender batch must be between one and eight frames",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid scheduler scenario: media datagram size is outside the protocol bound",
            ),
            Self::TargetFrameDurationMismatch => formatter.write_str(
                "invalid scheduler scenario: controller frame duration does not match scheduling",
            ),
            Self::TargetBufferCapacityMismatch => formatter.write_str(
                "invalid scheduler scenario: controller capacity does not match the jitter buffer",
            ),
            Self::Target(error) => {
                write!(formatter, "invalid scheduler scenario target: {error}")
            }
            Self::Profile(error) => {
                write!(formatter, "invalid scheduler scenario profile: {error}")
            }
            Self::Clock(error) => write!(formatter, "invalid scheduler scenario clock: {error}"),
            Self::InternalCapacityInvariant => formatter
                .write_str("invalid scheduler scenario: internal capacity invariant was violated"),
            Self::SimulationDidNotFinish => formatter.write_str(
                "invalid scheduler scenario: the hard deadline elapsed before completion",
            ),
        }
    }
}

impl std::error::Error for JitterSchedulerScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::InvalidFrameCount
            | Self::InvalidFrameDuration
            | Self::InvalidInitialPlayout
            | Self::InvalidMaxGateTicks
            | Self::InvalidSenderBatch
            | Self::InvalidMediaDatagramBytes
            | Self::TargetFrameDurationMismatch
            | Self::TargetBufferCapacityMismatch
            | Self::InternalCapacityInvariant
            | Self::SimulationDidNotFinish => None,
        }
    }
}

/// Runs a deterministic target-gated playout scheduler simulation.
///
/// The sender emits one frame per frame duration on the virtual link while the
/// scheduler ticks at the same cadence starting at `initial_playout_ms`. Each
/// tick the scheduler plays out only when at least the adaptive target frames
/// are buffered, waits while the target is unmet and arrivals are still
/// pending, drains without gating once no frames are in flight, and forces a
/// playout when the gate deadline expires. The controller observes the
/// arrival-jitter deviation and the pre-decision water level once per tick.
///
/// # Errors
///
/// Returns [`JitterSchedulerScenarioError`] when the configuration or the
/// controller target is outside an explicit bound, when the controller
/// configuration does not match the transport frame duration or buffer
/// capacity, when virtual time overflows, or when the hard deadline elapses
/// before every frame is consumed.
#[allow(clippy::too_many_lines)]
pub fn simulate_jitter_scheduler(
    config: JitterSchedulerScenarioConfig,
) -> Result<JitterSchedulerSimulationReport, JitterSchedulerScenarioError> {
    simulate_jitter_scheduler_batched(config, 1)
}

/// Runs the scheduler with frames released in batches at the final frame's capture time.
///
/// This models burst release only, not a sender queue or a pacing policy. The
/// final partial batch is released at its final frame's capture time.
///
/// # Errors
///
/// Returns the same errors as [`simulate_jitter_scheduler`], or
/// [`JitterSchedulerScenarioError::InvalidSenderBatch`] for a batch outside 1..=8.
#[allow(clippy::too_many_lines)]
pub fn simulate_jitter_scheduler_batched(
    config: JitterSchedulerScenarioConfig,
    batch_frames: u32,
) -> Result<JitterSchedulerSimulationReport, JitterSchedulerScenarioError> {
    if !(1..=8).contains(&batch_frames) {
        return Err(JitterSchedulerScenarioError::InvalidSenderBatch);
    }
    let frame = validate(config)?;
    let mut target =
        AdaptiveJitterTarget::new(config.target).map_err(JitterSchedulerScenarioError::Target)?;
    let mut network = VirtualNetwork::<u32>::new(config.network)
        .map_err(JitterSchedulerScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitter = JitterBuffer::<JITTER_BUFFER_CAPACITY, 8>::with_start_sequence(0);
    let mut report = JitterSchedulerSimulationReport {
        network: NetworkStats::default(),
        playout: SimulationReport::default(),
        final_target_frames: 0,
        min_frames: target.min_frames(),
        base_frames: target.base_frames(),
        max_frames: target.max_frames(),
        final_smoothed_jitter_ms: 0,
        raised_steps: 0,
        lowered_steps: 0,
        observations: 0,
        observed_jitter_max_ms: 0,
        final_buffered_frames: 0,
        gated_ticks: 0,
        forced_playouts: 0,
        excess_latency_ms: 0,
    };
    let mut state = SchedulerState {
        consumed_frames: 0,
        playout: TargetGatedPlayout::new(TargetGatedPlayoutConfig {
            max_gate_ticks: config.max_gate_ticks,
        })
        .map_err(|_| JitterSchedulerScenarioError::InternalCapacityInvariant)?,
    };
    let mut last_arrival_jitter_ms = 0_u32;
    let mut next_send_sequence = 0_u32;
    let mut next_send_ms =
        u64::from(batch_frames.min(config.frames) - 1) * config.frame_duration_ms;
    let mut next_playout_ms = config.initial_playout_ms;
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
    let mut completion_ms: Option<u64> = None;

    while state.consumed_frames < config.frames && clock.now_ms() <= hard_end_ms {
        let now_ms = clock.now_ms();
        if next_send_sequence < config.frames && now_ms == next_send_ms {
            let batch_end = next_send_sequence
                .saturating_add(batch_frames)
                .min(config.frames);
            while next_send_sequence < batch_end {
                network.send(
                    now_ms,
                    next_send_sequence,
                    u64::from(config.media_datagram_bytes),
                );
                next_send_sequence += 1;
            }
            if next_send_sequence < config.frames {
                let final_sequence = next_send_sequence
                    .saturating_add(batch_frames)
                    .min(config.frames)
                    - 1;
                next_send_ms = u64::from(final_sequence) * config.frame_duration_ms;
            }
        }

        for delivery in network.drain_ready(now_ms) {
            record_delivery(
                &mut report,
                &mut jitter,
                &mut last_arrival_jitter_ms,
                config.network.base_delay_ms,
                frame,
                &delivery,
            )?;
        }

        if now_ms == next_playout_ms {
            let arrivals_pending = next_send_sequence < config.frames
                || network.stats().scheduled_copies > network.stats().delivered_copies;
            apply_playout_tick(
                &mut report,
                &mut target,
                &mut jitter,
                last_arrival_jitter_ms,
                arrivals_pending,
                &mut state,
            )?;
            if state.consumed_frames == config.frames && completion_ms.is_none() {
                completion_ms = Some(now_ms);
            }
            next_playout_ms = next_playout_ms.saturating_add(config.frame_duration_ms);
        }
        clock
            .advance_by(1)
            .map_err(JitterSchedulerScenarioError::Clock)?;
    }

    report.network = network.stats();
    report.gated_ticks = u32::try_from(state.playout.gated_ticks()).unwrap_or(u32::MAX);
    report.forced_playouts = u32::try_from(state.playout.forced_playouts()).unwrap_or(u32::MAX);
    report.final_target_frames = target.target_frames();
    report.final_smoothed_jitter_ms = target.smoothed_jitter_ms();
    report.final_buffered_frames = jitter.len();
    if state.consumed_frames < config.frames {
        return Err(JitterSchedulerScenarioError::SimulationDidNotFinish);
    }
    report.excess_latency_ms = completion_ms
        .unwrap_or(nominal_completion_ms)
        .saturating_sub(nominal_completion_ms);
    Ok(report)
}

/// Mutable scheduler bookkeeping shared by one playout tick.
struct SchedulerState {
    consumed_frames: u32,
    playout: TargetGatedPlayout,
}

/// Records one delivered encoded frame and updates the transport counters.
fn record_delivery(
    report: &mut JitterSchedulerSimulationReport,
    jitter: &mut JitterBuffer<JITTER_BUFFER_CAPACITY, 8>,
    last_arrival_jitter_ms: &mut u32,
    base_delay_ms: u64,
    frame: OpusFrameConfig,
    delivery: &Delivery<u32>,
) -> Result<(), JitterSchedulerScenarioError> {
    let sequence = delivery.payload;
    let delay_ms = delivery.delivered_at_ms.saturating_sub(delivery.sent_at_ms);
    let arrival_jitter_ms = u32::try_from(delay_ms.abs_diff(base_delay_ms)).unwrap_or(u32::MAX);
    *last_arrival_jitter_ms = arrival_jitter_ms;
    report.observed_jitter_max_ms = report.observed_jitter_max_ms.max(arrival_jitter_ms);
    let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
        .map_err(|_| JitterSchedulerScenarioError::InternalCapacityInvariant)?;
    match jitter.insert(encoded) {
        InsertOutcome::Inserted => {}
        InsertOutcome::Reordered => report.playout.reordered_arrivals += 1,
        InsertOutcome::Duplicate => report.playout.duplicate_frames += 1,
        InsertOutcome::Late => report.playout.late_frames += 1,
        InsertOutcome::Resynchronized { .. } => report.playout.resynchronizations += 1,
        InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
            return Err(JitterSchedulerScenarioError::InternalCapacityInvariant);
        }
    }
    Ok(())
}

/// Applies one target-gated scheduler tick and feeds the adaptive target.
///
/// The target-gated decision and its counters live in the shared
/// [`TargetGatedPlayout`] primitive; this function plays out one consumed
/// frame (or conceals a missing one) on a play decision.
fn apply_playout_tick(
    report: &mut JitterSchedulerSimulationReport,
    target: &mut AdaptiveJitterTarget,
    jitter: &mut JitterBuffer<JITTER_BUFFER_CAPACITY, 8>,
    arrival_jitter_ms: u32,
    arrivals_pending: bool,
    state: &mut SchedulerState,
) -> Result<(), JitterSchedulerScenarioError> {
    let buffered = jitter.len();
    let target_frames = target.target_frames();
    if matches!(
        state
            .playout
            .decide(buffered, target_frames, arrivals_pending),
        PlayoutDecision::Play
    ) {
        match jitter.take_next() {
            PlayoutOutcome::Frame(_) => report.playout.playout_frames += 1,
            PlayoutOutcome::Missing { .. } => report.playout.plc_frames += 1,
            // Playout is pre-armed at the negotiated sequence, so this
            // no-op is defensive and unreachable.
            PlayoutOutcome::NotReady => {
                return Err(JitterSchedulerScenarioError::InternalCapacityInvariant);
            }
        }
        state.consumed_frames += 1;
    }
    report.observations += 1;
    match target.observe(arrival_jitter_ms, buffered) {
        JitterTargetStep::Raised => report.raised_steps += 1,
        JitterTargetStep::Lowered => report.lowered_steps += 1,
        JitterTargetStep::Held => {}
    }
    Ok(())
}

fn validate(
    config: JitterSchedulerScenarioConfig,
) -> Result<OpusFrameConfig, JitterSchedulerScenarioError> {
    if config.frames == 0 || config.frames > MAX_SCHEDULER_FRAMES {
        return Err(JitterSchedulerScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| JitterSchedulerScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| JitterSchedulerScenarioError::InvalidFrameDuration)?;
    if config.target.frame_duration_ms != u32::from(frame_duration_ms) {
        return Err(JitterSchedulerScenarioError::TargetFrameDurationMismatch);
    }
    config
        .target
        .validate()
        .map_err(JitterSchedulerScenarioError::Target)?;
    if config.target.buffer_capacity_frames != JITTER_BUFFER_CAPACITY {
        return Err(JitterSchedulerScenarioError::TargetBufferCapacityMismatch);
    }
    if config.initial_playout_ms > 1_000 {
        return Err(JitterSchedulerScenarioError::InvalidInitialPlayout);
    }
    if config.max_gate_ticks == 0 || config.max_gate_ticks > MAX_GATE_TICKS_BOUND {
        return Err(JitterSchedulerScenarioError::InvalidMaxGateTicks);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(JitterSchedulerScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(JitterSchedulerScenarioError::Profile)?;
    Ok(frame)
}

fn sample_timestamp(sequence: u32, frame: OpusFrameConfig) -> u64 {
    u64::from(sequence).saturating_mul(frame.samples_per_channel() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_media_datagrams_at_the_protocol_bound() {
        let config = JitterSchedulerScenarioConfig {
            media_datagram_bytes: u16::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap(),
            ..JitterSchedulerScenarioConfig::default()
        };

        assert!(simulate_jitter_scheduler(config).is_ok());
    }

    #[test]
    fn max_delay_profile_completes_within_the_hard_deadline() {
        let mut network = NetworkProfile::stable(0x5A17_2026);
        network.base_delay_ms = crate::MAX_NETWORK_DELAY_MS;
        network.jitter_ms = 0;
        network.reorder_delay_ms = 0;
        let config = JitterSchedulerScenarioConfig {
            media_datagram_bytes: u16::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap(),
            network,
            ..JitterSchedulerScenarioConfig::default()
        };

        let report = simulate_jitter_scheduler(config).expect("max-delay scenario must finish");
        assert!(
            report.forced_playouts > 0,
            "the bounded gate budget must force playout instead of stalling"
        );
        assert_eq!(
            report.playout.playout_frames, 0,
            "every arrival is late once the bounded gate has advanced past it"
        );
        assert_eq!(
            report.playout.plc_frames, config.frames,
            "every frame is concealed rather than left unaccounted"
        );
        assert!(
            report.excess_latency_ms <= crate::MAX_NETWORK_DELAY_MS,
            "added completion latency stays within the maximum one-way delay: {}",
            report.excess_latency_ms
        );
    }
}
