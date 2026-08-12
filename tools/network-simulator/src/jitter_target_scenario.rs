//! Deterministic adaptive-jitter-target simulation.
//!
//! One source stream traverses a [`VirtualNetwork`] while a per-stream
//! [`AdaptiveJitterTarget`] (ADR 0016) observes the link's arrival jitter and
//! the encoded-frame water level once per playout decision. The report carries
//! the bounded target trajectory (final target, raised/lowered steps,
//! smoothed jitter) as deterministic jitter-control evidence; it makes no
//! physical latency claim.

use std::fmt;

use sonimesh_audio::{
    AdaptiveJitterTarget, EncodedFrame, InsertOutcome, JitterBuffer, JitterMode,
    JitterTargetConfig, JitterTargetConfigError, JitterTargetStep, OPUS_CHANNELS,
    OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutOutcome,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::{
    ClockError, NetworkProfile, NetworkStats, ProfileError, SimulationReport, VirtualClock,
    VirtualNetwork,
};

/// Hard bound on sender frames in the jitter-target scenario.
const MAX_JITTER_SCENARIO_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of the scenario's jitter buffer.
const JITTER_BUFFER_CAPACITY: usize = 64;

/// One deterministic adaptive-jitter-target simulation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterTargetScenarioConfig {
    /// Number of sender frames.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and receiver scheduling.
    pub frame_duration_ms: u64,
    /// Fixed initial playout delay.
    pub target_buffer_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Virtual network impairments that generate the arrival-jitter signal.
    pub network: NetworkProfile,
    /// Bounded controller configuration; its frame duration and buffer capacity
    /// must match the scheduling frame duration and jitter buffer capacity.
    pub target: JitterTargetConfig,
}

impl Default for JitterTargetScenarioConfig {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_BUFFER_CAPACITY),
        }
    }
}

/// Deterministic transport and adaptive-target simulation counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterTargetSimulationReport {
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
    /// Controller observations, one per playout decision.
    pub observations: u32,
    /// Largest raw arrival-jitter deviation observed, in milliseconds.
    pub observed_jitter_max_ms: u32,
    /// Encoded frames buffered at the final playout decision.
    pub final_buffered_frames: usize,
}

/// Invalid jitter-target simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitterTargetScenarioError {
    /// Frame count must be between one and the scenario hard bound.
    InvalidFrameCount,
    /// Frame duration must be one negotiated version-one value: 10 ms or 20 ms.
    InvalidFrameDuration,
    /// Initial playout target must be no more than one second.
    InvalidTargetBuffer,
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
}

impl fmt::Display for JitterTargetScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameCount => formatter.write_str(
                "invalid jitter-target scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid jitter-target scenario: frame duration is not a negotiated value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid jitter-target scenario: target buffer is outside the explicit bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid jitter-target scenario: media datagram size is outside the protocol bound",
            ),
            Self::TargetFrameDurationMismatch => formatter.write_str(
                "invalid jitter-target scenario: controller frame duration does not match scheduling",
            ),
            Self::TargetBufferCapacityMismatch => formatter.write_str(
                "invalid jitter-target scenario: controller buffer capacity does not match the buffer",
            ),
            Self::Target(error) => {
                write!(
                    formatter,
                    "invalid jitter-target scenario target configuration: {error}"
                )
            }
            Self::Profile(error) => {
                write!(
                    formatter,
                    "invalid jitter-target scenario network profile: {error}"
                )
            }
            Self::Clock(error) => {
                write!(
                    formatter,
                    "invalid jitter-target scenario clock operation: {error}"
                )
            }
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid jitter-target scenario: internal capacity invariant was violated",
            ),
        }
    }
}

impl std::error::Error for JitterTargetScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::InvalidFrameCount
            | Self::InvalidFrameDuration
            | Self::InvalidTargetBuffer
            | Self::InvalidMediaDatagramBytes
            | Self::TargetFrameDurationMismatch
            | Self::TargetBufferCapacityMismatch
            | Self::InternalCapacityInvariant => None,
        }
    }
}

/// Runs a deterministic adaptive-jitter-target simulation.
///
/// One source stream sends on the virtual link while the controller observes
/// the absolute one-way-delay deviation from the profile base delay and the
/// encoded-frame water level once per playout decision.
///
/// # Errors
///
/// Returns [`JitterTargetScenarioError`] when the configuration or the
/// controller target is outside an explicit bound, when the controller
/// configuration does not match the transport frame duration or buffer
/// capacity, or when virtual time overflows.
pub fn simulate_jitter_target(
    config: JitterTargetScenarioConfig,
) -> Result<JitterTargetSimulationReport, JitterTargetScenarioError> {
    let frame = validate(config)?;
    let mut target =
        AdaptiveJitterTarget::new(config.target).map_err(JitterTargetScenarioError::Target)?;
    let mut network =
        VirtualNetwork::<u32>::new(config.network).map_err(JitterTargetScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitter = JitterBuffer::<64, 8>::with_start_sequence(0);
    let mut report = JitterTargetSimulationReport {
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
    };
    let mut last_arrival_jitter_ms = 0_u32;
    let mut next_send_sequence = 0_u32;
    let mut next_send_ms = 0_u64;
    let mut next_playout_ms = config.target_buffer_ms;
    let mut playout_decisions = 0_u32;
    let last_playout_ms = config.target_buffer_ms.saturating_add(
        u64::from(config.frames.saturating_sub(1)).saturating_mul(config.frame_duration_ms),
    );
    let end_ms = last_playout_ms
        .saturating_add(config.network.base_delay_ms)
        .saturating_add(config.network.jitter_ms)
        .saturating_add(config.network.reorder_delay_ms)
        .saturating_add(2);

    while clock.now_ms() <= end_ms {
        let now_ms = clock.now_ms();
        if next_send_sequence < config.frames && now_ms == next_send_ms {
            network.send(
                now_ms,
                next_send_sequence,
                u64::from(config.media_datagram_bytes),
            );
            next_send_sequence += 1;
            next_send_ms = next_send_ms.saturating_add(config.frame_duration_ms);
        }

        for delivery in network.drain_ready(now_ms) {
            let sequence = delivery.payload;
            let delay_ms = delivery.delivered_at_ms.saturating_sub(delivery.sent_at_ms);
            let arrival_jitter_ms =
                u32::try_from(delay_ms.abs_diff(config.network.base_delay_ms)).unwrap_or(u32::MAX);
            last_arrival_jitter_ms = arrival_jitter_ms;
            report.observed_jitter_max_ms = report.observed_jitter_max_ms.max(arrival_jitter_ms);
            let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| JitterTargetScenarioError::InternalCapacityInvariant)?;
            match jitter.insert(encoded) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => report.playout.reordered_arrivals += 1,
                InsertOutcome::Duplicate => report.playout.duplicate_frames += 1,
                InsertOutcome::Late => report.playout.late_frames += 1,
                InsertOutcome::Resynchronized { .. } => report.playout.resynchronizations += 1,
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(JitterTargetScenarioError::InternalCapacityInvariant);
                }
            }
        }

        if playout_decisions < config.frames && now_ms == next_playout_ms {
            match jitter.take_next() {
                PlayoutOutcome::Frame(_) => report.playout.playout_frames += 1,
                PlayoutOutcome::Missing { .. } => report.playout.plc_frames += 1,
                PlayoutOutcome::NotReady => {
                    return Err(JitterTargetScenarioError::InvalidTargetBuffer);
                }
            }
            playout_decisions += 1;
            next_playout_ms = next_playout_ms.saturating_add(config.frame_duration_ms);
            report.observations += 1;
            match target.observe(last_arrival_jitter_ms, jitter.len()) {
                JitterTargetStep::Raised => report.raised_steps += 1,
                JitterTargetStep::Lowered => report.lowered_steps += 1,
                JitterTargetStep::Held => {}
            }
        }
        clock
            .advance_by(1)
            .map_err(JitterTargetScenarioError::Clock)?;
    }

    report.network = network.stats();
    report.final_target_frames = target.target_frames();
    report.final_smoothed_jitter_ms = target.smoothed_jitter_ms();
    report.final_buffered_frames = jitter.len();
    Ok(report)
}

fn validate(
    config: JitterTargetScenarioConfig,
) -> Result<OpusFrameConfig, JitterTargetScenarioError> {
    if config.frames == 0 || config.frames > MAX_JITTER_SCENARIO_FRAMES {
        return Err(JitterTargetScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| JitterTargetScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| JitterTargetScenarioError::InvalidFrameDuration)?;
    if config.target.frame_duration_ms != u32::from(frame_duration_ms) {
        return Err(JitterTargetScenarioError::TargetFrameDurationMismatch);
    }
    config
        .target
        .validate()
        .map_err(JitterTargetScenarioError::Target)?;
    if config.target.buffer_capacity_frames != JITTER_BUFFER_CAPACITY {
        return Err(JitterTargetScenarioError::TargetBufferCapacityMismatch);
    }
    if config.target_buffer_ms > 1_000 {
        return Err(JitterTargetScenarioError::InvalidTargetBuffer);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(JitterTargetScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(JitterTargetScenarioError::Profile)?;
    Ok(frame)
}

fn sample_timestamp(sequence: u32, frame: OpusFrameConfig) -> u64 {
    u64::from(sequence).saturating_mul(frame.samples_per_channel() as u64)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{ClockError, MAX_BANDWIDTH_BYTES_PER_SECOND, ProfileError};

    use super::*;

    fn balanced_target(frame_duration_ms: u32) -> JitterTargetConfig {
        JitterTargetConfig::preset(
            JitterMode::Balanced,
            frame_duration_ms,
            JITTER_BUFFER_CAPACITY,
        )
    }

    #[test]
    fn simulation_is_deterministic_and_reports_a_bounded_target() {
        let mut network = NetworkProfile::stable(42);
        network.loss_percent = 3;
        network.duplicate_percent = 5;
        network.reorder_percent = 10;
        let config = JitterTargetScenarioConfig {
            frames: 200,
            network,
            ..JitterTargetScenarioConfig::default()
        };

        let first = simulate_jitter_target(config).unwrap();
        let second = simulate_jitter_target(config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.observations, config.frames);
        assert_eq!(
            first.playout.playout_frames + first.playout.plc_frames,
            config.frames
        );
        assert!(
            (first.min_frames..=first.max_frames).contains(&first.final_target_frames),
            "target left the bounded water-level range"
        );
        assert!(first.network.random_loss_packets > 0);
        assert!(first.playout.plc_frames > 0);
    }

    #[test]
    fn sustained_high_jitter_raises_the_target_toward_max() {
        let mut network = NetworkProfile::stable(7);
        network.reorder_percent = 100;
        network.reorder_delay_ms = 120;
        let config = JitterTargetScenarioConfig {
            frames: 100,
            target_buffer_ms: 200,
            network,
            ..JitterTargetScenarioConfig::default()
        };

        let report = simulate_jitter_target(config).unwrap();

        assert_eq!(report.final_target_frames, report.max_frames);
        assert!(report.raised_steps > 0);
        assert!(report.observed_jitter_max_ms >= 100);
        assert_eq!(
            report.playout.playout_frames + report.playout.plc_frames,
            config.frames
        );
    }

    #[test]
    fn sustained_low_jitter_with_surplus_lowers_the_target_toward_min() {
        let mut network = NetworkProfile::stable(9);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        let config = JitterTargetScenarioConfig {
            frames: 100,
            target_buffer_ms: 100,
            network,
            ..JitterTargetScenarioConfig::default()
        };

        let report = simulate_jitter_target(config).unwrap();

        assert_eq!(report.final_target_frames, report.min_frames);
        assert!(report.lowered_steps > 0);
        assert_eq!(report.raised_steps, 0);
        assert_eq!(
            report.playout.playout_frames + report.playout.plc_frames,
            config.frames
        );
    }

    #[test]
    fn stable_profile_holds_the_mode_base() {
        let config = JitterTargetScenarioConfig {
            frames: 200,
            ..JitterTargetScenarioConfig::default()
        };

        let report = simulate_jitter_target(config).unwrap();

        assert_eq!(report.final_target_frames, report.base_frames);
        assert_eq!(report.raised_steps, 0);
        assert_eq!(report.lowered_steps, 0);
        assert_eq!(report.playout.playout_frames, config.frames);
        assert_eq!(report.playout.plc_frames, 0);
    }

    #[test]
    fn rejects_invalid_configuration() {
        let invalid_frames = JitterTargetScenarioConfig {
            frames: 0,
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(invalid_frames),
            Err(JitterTargetScenarioError::InvalidFrameCount)
        );

        let unnegotiated_duration = JitterTargetScenarioConfig {
            frame_duration_ms: 15,
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(unnegotiated_duration),
            Err(JitterTargetScenarioError::InvalidFrameDuration)
        );

        let mismatched_duration = JitterTargetScenarioConfig {
            target: balanced_target(10),
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(mismatched_duration),
            Err(JitterTargetScenarioError::TargetFrameDurationMismatch)
        );

        let mismatched_capacity = JitterTargetScenarioConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 8),
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(mismatched_capacity),
            Err(JitterTargetScenarioError::TargetBufferCapacityMismatch)
        );

        let zero_capacity = JitterTargetScenarioConfig {
            target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 0),
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(zero_capacity),
            Err(JitterTargetScenarioError::Target(
                JitterTargetConfigError::ZeroCapacity
            ))
        );

        let oversized_buffer = JitterTargetScenarioConfig {
            target_buffer_ms: 1_001,
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(oversized_buffer),
            Err(JitterTargetScenarioError::InvalidTargetBuffer)
        );

        let zero_datagram = JitterTargetScenarioConfig {
            media_datagram_bytes: 0,
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(zero_datagram),
            Err(JitterTargetScenarioError::InvalidMediaDatagramBytes)
        );

        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        let invalid_profile = JitterTargetScenarioConfig {
            network,
            ..JitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_target(invalid_profile),
            Err(JitterTargetScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let target = JitterTargetScenarioError::Target(JitterTargetConfigError::ZeroCapacity);
        assert!(
            Error::source(&target)
                .and_then(|source| source.downcast_ref::<JitterTargetConfigError>())
                .is_some()
        );

        let profile = JitterTargetScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = JitterTargetScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&JitterTargetScenarioError::InvalidFrameCount).is_none());
        assert!(Error::source(&JitterTargetScenarioError::TargetFrameDurationMismatch).is_none());
        assert!(Error::source(&JitterTargetScenarioError::InternalCapacityInvariant).is_none());
    }
}
