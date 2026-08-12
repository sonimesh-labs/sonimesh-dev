//! Deterministic multi-stream adaptive-jitter-target simulation.
//!
//! Two to four source streams traverse one [`VirtualNetwork`] with a single
//! [`NetworkProfile`], so burst loss and finite-bandwidth serialization are
//! correlated across streams exactly as they would be on one shared WLAN link.
//! Unlike the plain multi-stream scenario, each stream additionally owns an
//! independent ADR 0016 [`AdaptiveJitterTarget`] that observes that stream's
//! virtual-link arrival jitter and encoded-frame water level once per playout
//! decision. The report carries shared link counters plus a per-stream target
//! trajectory, so correlated shared-link impairments can be observed against
//! per-stream adaptive jitter control. It makes no physical latency claim.

use std::fmt;

use sonimesh_audio::{
    AdaptiveJitterTarget, EncodedFrame, InsertOutcome, JitterBuffer, JitterMode,
    JitterTargetConfig, JitterTargetConfigError, JitterTargetStep, OPUS_CHANNELS,
    OPUS_SAMPLE_RATE_HZ, OpusFrameConfig, PlayoutOutcome,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::jitter_target_scenario::JitterTargetSimulationReport;
use crate::multi_stream_scenario::{MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS};
use crate::{
    ClockError, NetworkProfile, NetworkStats, ProfileError, SimulationReport, VirtualClock,
    VirtualNetwork,
};

/// Hard bound on sender frames per stream in the jitter-target scenario.
const MAX_MULTI_STREAM_JITTER_TARGET_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;
/// Encoded-frame capacity of every stream's jitter buffer.
const JITTER_BUFFER_CAPACITY: usize = 64;

/// One deterministic multi-stream adaptive-jitter-target configuration.
///
/// `STREAMS` source streams share one link and profile while each stream
/// owns the `targets[stream]` controller configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultiStreamJitterTargetScenarioConfig<const STREAMS: usize> {
    /// Number of sender frames per stream.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and receiver scheduling.
    pub frame_duration_ms: u64,
    /// Fixed initial playout delay applied to every stream.
    pub target_buffer_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Shared virtual link impairments applied to every stream.
    pub network: NetworkProfile,
    /// Per-stream controller configurations; each must match the scheduling
    /// frame duration and the jitter buffer capacity.
    pub targets: [JitterTargetConfig; STREAMS],
}

impl<const STREAMS: usize> Default for MultiStreamJitterTargetScenarioConfig<STREAMS> {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
            targets: std::array::from_fn(|_| {
                JitterTargetConfig::preset(JitterMode::Balanced, 20, JITTER_BUFFER_CAPACITY)
            }),
        }
    }
}

/// Deterministic shared-link and per-stream adaptive-target counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultiStreamJitterTargetSimulationReport<const STREAMS: usize> {
    /// Shared virtual link counters for the one link all streams traverse.
    pub network: NetworkStats,
    /// Per-stream transport and adaptive-target counters; each entry's
    /// `network` field is zero.
    pub streams: [JitterTargetSimulationReport; STREAMS],
}

/// Invalid multi-stream adaptive-jitter-target configuration or virtual time
/// operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MultiStreamJitterTargetScenarioError {
    /// Stream count is outside the explicit two-to-four bound.
    InvalidStreamCount,
    /// Frame count must be between one and the scenario hard bound.
    InvalidFrameCount,
    /// Frame duration must be one negotiated version-one value: 10 ms or 20 ms.
    InvalidFrameDuration,
    /// Initial playout target must be no more than one second.
    InvalidTargetBuffer,
    /// Complete media datagram size must fit the version-one packet limit.
    InvalidMediaDatagramBytes,
    /// One controller frame duration does not match the scheduling frame duration.
    TargetFrameDurationMismatch,
    /// One controller buffer capacity does not match the jitter buffer capacity.
    TargetBufferCapacityMismatch,
    /// One adaptive target configuration is invalid.
    Target(JitterTargetConfigError),
    /// Network profile is invalid.
    Profile(ProfileError),
    /// Virtual time overflowed.
    Clock(ClockError),
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
}

impl fmt::Display for MultiStreamJitterTargetScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter.write_str(
                "invalid multi-stream jitter-target scenario: stream count is outside the explicit bound",
            ),
            Self::InvalidFrameCount => formatter.write_str(
                "invalid multi-stream jitter-target scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid multi-stream jitter-target scenario: frame duration is not a negotiated value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid multi-stream jitter-target scenario: target buffer is outside the explicit bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid multi-stream jitter-target scenario: media datagram size is outside the protocol bound",
            ),
            Self::TargetFrameDurationMismatch => formatter.write_str(
                "invalid multi-stream jitter-target scenario: controller frame duration does not match scheduling",
            ),
            Self::TargetBufferCapacityMismatch => formatter.write_str(
                "invalid multi-stream jitter-target scenario: controller buffer capacity does not match the buffer",
            ),
            Self::Target(error) => {
                write!(
                    formatter,
                    "invalid multi-stream jitter-target scenario target configuration: {error}"
                )
            }
            Self::Profile(error) => {
                write!(
                    formatter,
                    "invalid multi-stream jitter-target scenario network profile: {error}"
                )
            }
            Self::Clock(error) => {
                write!(
                    formatter,
                    "invalid multi-stream jitter-target scenario clock operation: {error}"
                )
            }
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid multi-stream jitter-target scenario: internal capacity invariant was violated",
            ),
        }
    }
}

impl std::error::Error for MultiStreamJitterTargetScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target(error) => Some(error),
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::InvalidStreamCount
            | Self::InvalidFrameCount
            | Self::InvalidFrameDuration
            | Self::InvalidTargetBuffer
            | Self::InvalidMediaDatagramBytes
            | Self::TargetFrameDurationMismatch
            | Self::TargetBufferCapacityMismatch
            | Self::InternalCapacityInvariant => None,
        }
    }
}

/// Runs a deterministic multi-stream adaptive-jitter-target simulation.
///
/// `STREAMS` source streams traverse one shared virtual link and profile. Each
/// stream sends its own sequence on the shared cadence, receives through its
/// own jitter buffer, plays out on its own deadline, and feeds its own
/// [`AdaptiveJitterTarget`] from that stream's arrival-jitter deviation and
/// encoded-frame water level at every playout decision.
///
/// # Errors
///
/// Returns [`MultiStreamJitterTargetScenarioError`] when `STREAMS` or the
/// configuration is outside an explicit bound, when any controller target does
/// not match the transport frame duration or buffer capacity, or when virtual
/// time overflows.
pub fn simulate_multi_stream_jitter_target<const STREAMS: usize>(
    config: MultiStreamJitterTargetScenarioConfig<STREAMS>,
) -> Result<MultiStreamJitterTargetSimulationReport<STREAMS>, MultiStreamJitterTargetScenarioError>
{
    let (frame, mut targets) = validate::<STREAMS>(config)?;
    let mut network = VirtualNetwork::<(u8, u32)>::new(config.network)
        .map_err(MultiStreamJitterTargetScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitters: [JitterBuffer<64, 8>; STREAMS] =
        std::array::from_fn(|_| JitterBuffer::<64, 8>::with_start_sequence(0));
    let mut streams: [JitterTargetSimulationReport; STREAMS] =
        std::array::from_fn(|stream| JitterTargetSimulationReport {
            network: NetworkStats::default(),
            playout: SimulationReport::default(),
            final_target_frames: 0,
            min_frames: targets[stream].min_frames(),
            base_frames: targets[stream].base_frames(),
            max_frames: targets[stream].max_frames(),
            final_smoothed_jitter_ms: 0,
            raised_steps: 0,
            lowered_steps: 0,
            observations: 0,
            observed_jitter_max_ms: 0,
            final_buffered_frames: 0,
        });
    let mut last_arrival_jitter_ms = [0_u32; STREAMS];
    let mut next_send_sequence = [0_u32; STREAMS];
    let mut next_send_ms = [0_u64; STREAMS];
    let mut next_playout_ms = [config.target_buffer_ms; STREAMS];
    let mut playout_decisions = [0_u32; STREAMS];
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
        for stream in 0..STREAMS {
            if next_send_sequence[stream] < config.frames && now_ms == next_send_ms[stream] {
                let tag = u8::try_from(stream)
                    .map_err(|_| MultiStreamJitterTargetScenarioError::InternalCapacityInvariant)?;
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
            let arrival_jitter_ms =
                u32::try_from(delay_ms.abs_diff(config.network.base_delay_ms)).unwrap_or(u32::MAX);
            last_arrival_jitter_ms[stream] = arrival_jitter_ms;
            streams[stream].observed_jitter_max_ms = streams[stream]
                .observed_jitter_max_ms
                .max(arrival_jitter_ms);
            let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| MultiStreamJitterTargetScenarioError::InternalCapacityInvariant)?;
            record_delivery(&mut streams[stream], &mut jitters[stream], encoded)?;
        }

        for stream in 0..STREAMS {
            if playout_decisions[stream] < config.frames && now_ms == next_playout_ms[stream] {
                apply_playout(
                    &mut streams[stream],
                    &mut targets[stream],
                    last_arrival_jitter_ms[stream],
                    &mut jitters[stream],
                )?;
                playout_decisions[stream] += 1;
                next_playout_ms[stream] =
                    next_playout_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }
        clock
            .advance_by(1)
            .map_err(MultiStreamJitterTargetScenarioError::Clock)?;
    }

    for stream in 0..STREAMS {
        streams[stream].final_target_frames = targets[stream].target_frames();
        streams[stream].final_smoothed_jitter_ms = targets[stream].smoothed_jitter_ms();
        streams[stream].final_buffered_frames = jitters[stream].len();
    }

    Ok(MultiStreamJitterTargetSimulationReport {
        network: network.stats(),
        streams,
    })
}

/// Records one delivered encoded frame and updates the stream transport
/// counters. The controller's observed-jitter maximum tracks every delivery.
fn record_delivery(
    report: &mut JitterTargetSimulationReport,
    jitter: &mut JitterBuffer<64, 8>,
    encoded: EncodedFrame<8>,
) -> Result<(), MultiStreamJitterTargetScenarioError> {
    match jitter.insert(encoded) {
        InsertOutcome::Inserted => {}
        InsertOutcome::Reordered => report.playout.reordered_arrivals += 1,
        InsertOutcome::Duplicate => report.playout.duplicate_frames += 1,
        InsertOutcome::Late => report.playout.late_frames += 1,
        InsertOutcome::Resynchronized { .. } => report.playout.resynchronizations += 1,
        InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
            return Err(MultiStreamJitterTargetScenarioError::InternalCapacityInvariant);
        }
    }
    Ok(())
}

/// Applies one playout decision and feeds the stream's adaptive target from
/// the latest arrival-jitter deviation and the remaining water level.
fn apply_playout(
    report: &mut JitterTargetSimulationReport,
    target: &mut AdaptiveJitterTarget,
    arrival_jitter_ms: u32,
    jitter: &mut JitterBuffer<64, 8>,
) -> Result<(), MultiStreamJitterTargetScenarioError> {
    match jitter.take_next() {
        PlayoutOutcome::Frame(_) => report.playout.playout_frames += 1,
        PlayoutOutcome::Missing { .. } => report.playout.plc_frames += 1,
        PlayoutOutcome::NotReady => {
            return Err(MultiStreamJitterTargetScenarioError::InvalidTargetBuffer);
        }
    }
    report.observations += 1;
    match target.observe(arrival_jitter_ms, jitter.len()) {
        JitterTargetStep::Raised => report.raised_steps += 1,
        JitterTargetStep::Lowered => report.lowered_steps += 1,
        JitterTargetStep::Held => {}
    }
    Ok(())
}

fn validate<const STREAMS: usize>(
    config: MultiStreamJitterTargetScenarioConfig<STREAMS>,
) -> Result<(OpusFrameConfig, [AdaptiveJitterTarget; STREAMS]), MultiStreamJitterTargetScenarioError>
{
    if !(MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS).contains(&STREAMS) {
        return Err(MultiStreamJitterTargetScenarioError::InvalidStreamCount);
    }
    if config.frames == 0 || config.frames > MAX_MULTI_STREAM_JITTER_TARGET_FRAMES {
        return Err(MultiStreamJitterTargetScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| MultiStreamJitterTargetScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| MultiStreamJitterTargetScenarioError::InvalidFrameDuration)?;
    for target in &config.targets {
        if target.frame_duration_ms != u32::from(frame_duration_ms) {
            return Err(MultiStreamJitterTargetScenarioError::TargetFrameDurationMismatch);
        }
        target
            .validate()
            .map_err(MultiStreamJitterTargetScenarioError::Target)?;
        if target.buffer_capacity_frames != JITTER_BUFFER_CAPACITY {
            return Err(MultiStreamJitterTargetScenarioError::TargetBufferCapacityMismatch);
        }
    }
    if config.target_buffer_ms > 1_000 {
        return Err(MultiStreamJitterTargetScenarioError::InvalidTargetBuffer);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(MultiStreamJitterTargetScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(MultiStreamJitterTargetScenarioError::Profile)?;
    let targets = std::array::from_fn(|stream| {
        AdaptiveJitterTarget::new(config.targets[stream])
            .expect("validated multi-stream jitter target configuration")
    });
    Ok((frame, targets))
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
    fn simulation_is_deterministic_and_reports_bounded_targets() {
        let mut network = NetworkProfile::stable(42);
        network.loss_percent = 3;
        network.duplicate_percent = 5;
        network.reorder_percent = 10;
        let config = MultiStreamJitterTargetScenarioConfig {
            frames: 200,
            network,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };

        let first = simulate_multi_stream_jitter_target::<2>(config).unwrap();
        let second = simulate_multi_stream_jitter_target::<2>(config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.streams.len(), 2);
        for stream in &first.streams {
            assert_eq!(stream.observations, config.frames);
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames
            );
            assert!(
                (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
                "target left the bounded water-level range"
            );
            assert_eq!(stream.network, NetworkStats::default());
        }
        assert!(first.network.random_loss_packets > 0);
        assert!(
            first
                .streams
                .iter()
                .any(|stream| stream.playout.plc_frames > 0)
        );
    }

    #[test]
    fn shared_burst_loss_impairs_every_stream_within_target_bounds() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = MultiStreamJitterTargetScenarioConfig {
            frames: 100,
            network,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };

        let report = simulate_multi_stream_jitter_target::<2>(config).unwrap();

        assert_eq!(report.network.burst_loss_packets, 27);
        for stream in &report.streams {
            assert!(stream.playout.plc_frames > 0);
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames
            );
            assert!(
                (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
                "target left the bounded water-level range"
            );
        }
    }

    #[test]
    fn per_stream_modes_keep_independent_water_levels() {
        let mut network = NetworkProfile::stable(9);
        network.loss_percent = 2;
        network.reorder_percent = 4;
        let config = MultiStreamJitterTargetScenarioConfig {
            frames: 200,
            network,
            targets: [
                JitterTargetConfig::preset(JitterMode::LowLatency, 20, JITTER_BUFFER_CAPACITY),
                JitterTargetConfig::preset(JitterMode::Stable, 20, JITTER_BUFFER_CAPACITY),
            ],
            ..MultiStreamJitterTargetScenarioConfig::default()
        };

        let report = simulate_multi_stream_jitter_target::<2>(config).unwrap();

        assert!(report.streams[0].max_frames < report.streams[1].max_frames);
        for stream in &report.streams {
            assert!(
                (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
                "target left the bounded water-level range"
            );
        }
    }

    #[test]
    fn clean_shared_link_holds_every_mode_base_across_four_streams() {
        let mut network = NetworkProfile::stable(0x0A0C_2026);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        let config = MultiStreamJitterTargetScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            target_buffer_ms: 10,
            network,
            targets: std::array::from_fn(|_| {
                JitterTargetConfig::preset(JitterMode::Balanced, 10, JITTER_BUFFER_CAPACITY)
            }),
            ..MultiStreamJitterTargetScenarioConfig::default()
        };

        let report = simulate_multi_stream_jitter_target::<4>(config).unwrap();

        for stream in &report.streams {
            assert_eq!(stream.playout.playout_frames, config.frames);
            assert_eq!(stream.playout.plc_frames, 0);
            assert_eq!(stream.final_target_frames, stream.base_frames);
            assert_eq!(stream.raised_steps, 0);
            assert_eq!(stream.lowered_steps, 0);
        }
    }

    #[test]
    fn rejects_invalid_transport_configuration() {
        let invalid_frames = MultiStreamJitterTargetScenarioConfig {
            frames: 0,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(invalid_frames),
            Err(MultiStreamJitterTargetScenarioError::InvalidFrameCount)
        );

        let unnegotiated_duration = MultiStreamJitterTargetScenarioConfig {
            frame_duration_ms: 15,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(unnegotiated_duration),
            Err(MultiStreamJitterTargetScenarioError::InvalidFrameDuration)
        );

        let oversized_buffer = MultiStreamJitterTargetScenarioConfig {
            target_buffer_ms: 1_001,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(oversized_buffer),
            Err(MultiStreamJitterTargetScenarioError::InvalidTargetBuffer)
        );

        let zero_datagram = MultiStreamJitterTargetScenarioConfig {
            media_datagram_bytes: 0,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(zero_datagram),
            Err(MultiStreamJitterTargetScenarioError::InvalidMediaDatagramBytes)
        );

        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        let invalid_profile = MultiStreamJitterTargetScenarioConfig {
            network,
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(invalid_profile),
            Err(MultiStreamJitterTargetScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
    }

    #[test]
    fn rejects_invalid_target_configuration() {
        let mismatched_duration = MultiStreamJitterTargetScenarioConfig {
            targets: [balanced_target(10), balanced_target(20)],
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(mismatched_duration),
            Err(MultiStreamJitterTargetScenarioError::TargetFrameDurationMismatch)
        );

        let mismatched_capacity = MultiStreamJitterTargetScenarioConfig {
            targets: [
                JitterTargetConfig::preset(JitterMode::Balanced, 20, 8),
                balanced_target(20),
            ],
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(mismatched_capacity),
            Err(MultiStreamJitterTargetScenarioError::TargetBufferCapacityMismatch)
        );

        let zero_capacity = MultiStreamJitterTargetScenarioConfig {
            targets: [
                JitterTargetConfig::preset(JitterMode::Balanced, 20, 0),
                balanced_target(20),
            ],
            ..MultiStreamJitterTargetScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream_jitter_target::<2>(zero_capacity),
            Err(MultiStreamJitterTargetScenarioError::Target(
                JitterTargetConfigError::ZeroCapacity
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let target =
            MultiStreamJitterTargetScenarioError::Target(JitterTargetConfigError::ZeroCapacity);
        assert!(
            Error::source(&target)
                .and_then(|source| source.downcast_ref::<JitterTargetConfigError>())
                .is_some()
        );

        let profile = MultiStreamJitterTargetScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = MultiStreamJitterTargetScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&MultiStreamJitterTargetScenarioError::InvalidStreamCount).is_none());
        assert!(
            Error::source(&MultiStreamJitterTargetScenarioError::TargetFrameDurationMismatch)
                .is_none()
        );
        assert!(
            Error::source(&MultiStreamJitterTargetScenarioError::InternalCapacityInvariant)
                .is_none()
        );
    }
}
