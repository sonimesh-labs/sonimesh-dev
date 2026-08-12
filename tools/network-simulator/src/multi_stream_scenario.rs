//! Deterministic multi-stream shared-link simulation.
//!
//! Two to four source streams traverse one [`VirtualNetwork`] with a single
//! [`NetworkProfile`], so burst loss and finite-bandwidth serialization are
//! correlated across streams exactly as they would be on one shared WLAN link.
//! Each stream still owns an independent [`JitterBuffer`], send cadence, and
//! playout deadline; the report carries shared link counters plus per-stream
//! jitter and PLC counters.

use std::fmt;

use sonimesh_audio::{
    EncodedFrame, InsertOutcome, JitterBuffer, OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ, OpusFrameConfig,
    PlayoutOutcome,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::{
    ClockError, NetworkProfile, NetworkStats, ProfileError, SimulationReport, VirtualClock,
    VirtualNetwork,
};

/// Fewest source streams accepted by the multi-stream scenario.
pub const MIN_MULTI_STREAM_STREAMS: usize = 2;
/// Most source streams accepted by the multi-stream scenario.
pub const MAX_MULTI_STREAM_STREAMS: usize = 4;
/// Hard bound on sender frames per stream.
const MAX_MULTI_STREAM_FRAMES: u32 = 100_000;
/// Default complete media datagram size in bytes.
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;

/// One deterministic multi-stream simulation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultiStreamScenarioConfig {
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
}

impl Default for MultiStreamScenarioConfig {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
            media_datagram_bytes: DEFAULT_MEDIA_DATAGRAM_BYTES,
            network: NetworkProfile::default(),
        }
    }
}

/// Deterministic per-stream and shared-link simulation counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultiStreamSimulationReport<const STREAMS: usize> {
    /// Shared virtual link counters for the one link all streams traverse.
    pub network: NetworkStats,
    /// Per-stream jitter and playout counters; each entry's `network` field is zero.
    pub streams: [SimulationReport; STREAMS],
}

/// Invalid multi-stream simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MultiStreamScenarioError {
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
    /// Network profile is invalid.
    Profile(ProfileError),
    /// Virtual time overflowed.
    Clock(ClockError),
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
}

impl fmt::Display for MultiStreamScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter.write_str(
                "invalid multi-stream scenario: stream count is outside the explicit bound",
            ),
            Self::InvalidFrameCount => formatter.write_str(
                "invalid multi-stream scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid multi-stream scenario: frame duration is not a negotiated value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid multi-stream scenario: target buffer is outside the explicit bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid multi-stream scenario: media datagram size is outside the protocol bound",
            ),
            Self::Profile(error) => {
                write!(
                    formatter,
                    "invalid multi-stream scenario network profile: {error}"
                )
            }
            Self::Clock(error) => {
                write!(
                    formatter,
                    "invalid multi-stream scenario clock operation: {error}"
                )
            }
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid multi-stream scenario: internal capacity invariant was violated",
            ),
        }
    }
}

impl std::error::Error for MultiStreamScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::InvalidStreamCount
            | Self::InvalidFrameCount
            | Self::InvalidFrameDuration
            | Self::InvalidTargetBuffer
            | Self::InvalidMediaDatagramBytes
            | Self::InternalCapacityInvariant => None,
        }
    }
}

/// Runs a deterministic multi-stream shared-link simulation.
///
/// `STREAMS` source streams traverse one shared virtual link and profile. Each
/// stream sends its own sequence on the shared cadence, receives through its
/// own jitter buffer, and plays out on its own deadline.
///
/// # Errors
///
/// Returns [`MultiStreamScenarioError`] when `STREAMS` or the configuration is
/// outside an explicit bound, or when virtual time overflows.
pub fn simulate_multi_stream<const STREAMS: usize>(
    config: MultiStreamScenarioConfig,
) -> Result<MultiStreamSimulationReport<STREAMS>, MultiStreamScenarioError> {
    let frame = validate::<STREAMS>(config)?;
    let mut network = VirtualNetwork::<(u8, u32)>::new(config.network)
        .map_err(MultiStreamScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitters: [JitterBuffer<64, 8>; STREAMS] =
        std::array::from_fn(|_| JitterBuffer::<64, 8>::with_start_sequence(0));
    let mut streams = std::array::from_fn(|_| SimulationReport::default());
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
                    .map_err(|_| MultiStreamScenarioError::InternalCapacityInvariant)?;
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
            let encoded = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| MultiStreamScenarioError::InternalCapacityInvariant)?;
            match jitters[stream].insert(encoded) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => streams[stream].reordered_arrivals += 1,
                InsertOutcome::Duplicate => streams[stream].duplicate_frames += 1,
                InsertOutcome::Late => streams[stream].late_frames += 1,
                InsertOutcome::Resynchronized { .. } => streams[stream].resynchronizations += 1,
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(MultiStreamScenarioError::InternalCapacityInvariant);
                }
            }
        }

        for stream in 0..STREAMS {
            if playout_decisions[stream] < config.frames && now_ms == next_playout_ms[stream] {
                match jitters[stream].take_next() {
                    PlayoutOutcome::Frame(_) => streams[stream].playout_frames += 1,
                    PlayoutOutcome::Missing { .. } => streams[stream].plc_frames += 1,
                    PlayoutOutcome::NotReady => {
                        return Err(MultiStreamScenarioError::InvalidTargetBuffer);
                    }
                }
                playout_decisions[stream] += 1;
                next_playout_ms[stream] =
                    next_playout_ms[stream].saturating_add(config.frame_duration_ms);
            }
        }
        clock
            .advance_by(1)
            .map_err(MultiStreamScenarioError::Clock)?;
    }

    Ok(MultiStreamSimulationReport {
        network: network.stats(),
        streams,
    })
}

fn validate<const STREAMS: usize>(
    config: MultiStreamScenarioConfig,
) -> Result<OpusFrameConfig, MultiStreamScenarioError> {
    if !(MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS).contains(&STREAMS) {
        return Err(MultiStreamScenarioError::InvalidStreamCount);
    }
    if config.frames == 0 || config.frames > MAX_MULTI_STREAM_FRAMES {
        return Err(MultiStreamScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms = u16::try_from(config.frame_duration_ms)
        .map_err(|_| MultiStreamScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| MultiStreamScenarioError::InvalidFrameDuration)?;
    if config.target_buffer_ms > 1_000 {
        return Err(MultiStreamScenarioError::InvalidTargetBuffer);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(MultiStreamScenarioError::InvalidMediaDatagramBytes);
    }
    config
        .network
        .validate()
        .map_err(MultiStreamScenarioError::Profile)?;
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

    fn impaired_profile(seed: u64) -> NetworkProfile {
        let mut network = NetworkProfile::stable(seed);
        network.loss_percent = 3;
        network.duplicate_percent = 5;
        network.reorder_percent = 10;
        network
    }

    #[test]
    fn stream_count_outside_the_explicit_bound_is_rejected() {
        let config = MultiStreamScenarioConfig::default();
        assert_eq!(
            simulate_multi_stream::<0>(config),
            Err(MultiStreamScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_multi_stream::<1>(config),
            Err(MultiStreamScenarioError::InvalidStreamCount)
        );
        assert_eq!(
            simulate_multi_stream::<5>(config),
            Err(MultiStreamScenarioError::InvalidStreamCount)
        );
    }

    #[test]
    fn simulation_is_deterministic_and_accounts_every_playout() {
        let config = MultiStreamScenarioConfig {
            frames: 200,
            network: impaired_profile(42),
            ..MultiStreamScenarioConfig::default()
        };

        let first = simulate_multi_stream::<2>(config).unwrap();
        let second = simulate_multi_stream::<2>(config).unwrap();

        assert_eq!(first, second);
        for stream in &first.streams {
            assert_eq!(stream.playout_frames + stream.plc_frames, config.frames);
            assert_eq!(stream.resynchronizations, 0);
        }
        assert!(first.network.random_loss_packets > 0);
        let total_plc: u32 = first.streams.iter().map(|stream| stream.plc_frames).sum();
        assert!(total_plc > 0);
    }

    #[test]
    fn shared_burst_loss_impairs_every_stream() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = MultiStreamScenarioConfig {
            frames: 100,
            network,
            ..MultiStreamScenarioConfig::default()
        };

        let report = simulate_multi_stream::<2>(config).unwrap();

        assert_eq!(report.network.burst_loss_packets, 27);
        assert_eq!(report.streams[0].plc_frames, 18);
        assert_eq!(report.streams[1].plc_frames, 9);
        for stream in &report.streams {
            assert_eq!(stream.playout_frames + stream.plc_frames, config.frames);
        }
    }

    #[test]
    fn shared_link_bandwidth_shapes_every_stream() {
        let mut network = NetworkProfile::stable(9);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        network.bandwidth_bytes_per_second = 1_000;
        let config = MultiStreamScenarioConfig {
            frames: 5,
            target_buffer_ms: 60,
            media_datagram_bytes: 100,
            network,
            ..MultiStreamScenarioConfig::default()
        };

        let report = simulate_multi_stream::<2>(config).unwrap();

        assert_eq!(report.network.bandwidth_shaped_copies, 10);
        assert_eq!(report.network.bandwidth_serialized_bytes, 1_000);
        for stream in &report.streams {
            assert_eq!(stream.playout_frames + stream.plc_frames, config.frames);
        }
        let total_plc: u32 = report.streams.iter().map(|stream| stream.plc_frames).sum();
        assert!(total_plc > 0);
    }

    #[test]
    fn clean_shared_link_plays_every_frame_across_four_streams() {
        let mut network = NetworkProfile::stable(0x0A0C_2026);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        let config = MultiStreamScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            target_buffer_ms: 10,
            network,
            ..MultiStreamScenarioConfig::default()
        };

        let report = simulate_multi_stream::<4>(config).unwrap();

        for stream in &report.streams {
            assert_eq!(stream.playout_frames, config.frames);
            assert_eq!(stream.plc_frames, 0);
        }
    }

    #[test]
    fn rejects_invalid_scenario_configuration() {
        let invalid_frames = MultiStreamScenarioConfig {
            frames: 0,
            ..MultiStreamScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream::<2>(invalid_frames),
            Err(MultiStreamScenarioError::InvalidFrameCount)
        );

        let unnegotiated_duration = MultiStreamScenarioConfig {
            frame_duration_ms: 15,
            ..MultiStreamScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream::<2>(unnegotiated_duration),
            Err(MultiStreamScenarioError::InvalidFrameDuration)
        );

        let oversized_buffer = MultiStreamScenarioConfig {
            target_buffer_ms: 1_001,
            ..MultiStreamScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream::<2>(oversized_buffer),
            Err(MultiStreamScenarioError::InvalidTargetBuffer)
        );

        let zero_datagram = MultiStreamScenarioConfig {
            media_datagram_bytes: 0,
            ..MultiStreamScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream::<2>(zero_datagram),
            Err(MultiStreamScenarioError::InvalidMediaDatagramBytes)
        );

        let mut network = NetworkProfile::stable(3);
        network.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;
        let invalid_profile = MultiStreamScenarioConfig {
            network,
            ..MultiStreamScenarioConfig::default()
        };
        assert_eq!(
            simulate_multi_stream::<2>(invalid_profile),
            Err(MultiStreamScenarioError::Profile(
                ProfileError::BandwidthTooLarge
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let profile = MultiStreamScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = MultiStreamScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&MultiStreamScenarioError::InvalidStreamCount).is_none());
        assert!(Error::source(&MultiStreamScenarioError::InternalCapacityInvariant).is_none());
    }
}
