use std::fmt;

use sonimesh_audio::{
    EncodedFrame, InsertOutcome, JitterBuffer, OPUS_CHANNELS, OPUS_SAMPLE_RATE_HZ, OpusFrameConfig,
    PlayoutOutcome,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

use crate::{ClockError, NetworkProfile, NetworkStats, ProfileError, VirtualClock, VirtualNetwork};

const MAX_SCENARIO_FRAMES: u32 = 100_000;
const DEFAULT_MEDIA_DATAGRAM_BYTES: u16 = 256;

/// One deterministic encoded-audio simulation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScenarioConfig {
    /// Number of sender frames.
    pub frames: u32,
    /// Negotiated 10 ms or 20 ms frame duration used by sender and receiver scheduling.
    pub frame_duration_ms: u64,
    /// Fixed initial playout delay.
    pub target_buffer_ms: u64,
    /// Complete on-wire media datagram bytes, including header and authentication tag.
    pub media_datagram_bytes: u16,
    /// Virtual network impairments.
    pub network: NetworkProfile,
}

impl Default for ScenarioConfig {
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

/// Deterministic simulation counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SimulationReport {
    /// Virtual transport counters.
    pub network: NetworkStats,
    /// Frames removed from jitter storage for decode.
    pub playout_frames: u32,
    /// Missing deadlines that require decoder-side PLC.
    pub plc_frames: u32,
    /// Duplicate frames rejected by jitter storage.
    pub duplicate_frames: u32,
    /// Frames arriving behind the playout sequence.
    pub late_frames: u32,
    /// Controlled stream-local jitter resets.
    pub resynchronizations: u32,
    /// Accepted arrivals whose sequence preceded an earlier arrival.
    pub reordered_arrivals: u32,
}

/// Invalid simulation configuration or virtual time operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScenarioError {
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

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFrameCount => formatter.write_str(
                "invalid deterministic scenario: frame count is outside the explicit bound",
            ),
            Self::InvalidFrameDuration => formatter.write_str(
                "invalid deterministic scenario: frame duration is not a negotiated value",
            ),
            Self::InvalidTargetBuffer => formatter.write_str(
                "invalid deterministic scenario: target buffer is outside the explicit bound",
            ),
            Self::InvalidMediaDatagramBytes => formatter.write_str(
                "invalid deterministic scenario: media datagram size is outside the protocol bound",
            ),
            Self::Profile(error) => {
                write!(
                    formatter,
                    "invalid deterministic scenario network profile: {error}"
                )
            }
            Self::Clock(error) => {
                write!(
                    formatter,
                    "invalid deterministic scenario clock operation: {error}"
                )
            }
            Self::InternalCapacityInvariant => formatter.write_str(
                "invalid deterministic scenario: internal capacity invariant was violated",
            ),
        }
    }
}

impl std::error::Error for ScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Profile(error) => Some(error),
            Self::Clock(error) => Some(error),
            Self::InvalidFrameCount
            | Self::InvalidFrameDuration
            | Self::InvalidTargetBuffer
            | Self::InvalidMediaDatagramBytes
            | Self::InternalCapacityInvariant => None,
        }
    }
}

/// Runs a deterministic encoded-frame and jitter-buffer scenario.
///
/// # Errors
///
/// Returns [`ScenarioError`] when configuration or virtual time is invalid.
pub fn simulate(config: ScenarioConfig) -> Result<SimulationReport, ScenarioError> {
    let frame = validate(config)?;
    let mut network = VirtualNetwork::new(config.network).map_err(ScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitter = JitterBuffer::<64, 8>::with_start_sequence(0);
    let mut report = SimulationReport::default();
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
            let frame = EncodedFrame::new(sequence, sample_timestamp(sequence, frame), &[0])
                .map_err(|_| ScenarioError::InternalCapacityInvariant)?;
            match jitter.insert(frame) {
                InsertOutcome::Inserted => {}
                InsertOutcome::Reordered => report.reordered_arrivals += 1,
                InsertOutcome::Duplicate => report.duplicate_frames += 1,
                InsertOutcome::Late => report.late_frames += 1,
                InsertOutcome::Resynchronized { .. } => report.resynchronizations += 1,
                InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
                    return Err(ScenarioError::InternalCapacityInvariant);
                }
            }
        }

        if playout_decisions < config.frames && now_ms == next_playout_ms {
            match jitter.take_next() {
                PlayoutOutcome::Frame(_) => report.playout_frames += 1,
                PlayoutOutcome::Missing { .. } => report.plc_frames += 1,
                PlayoutOutcome::NotReady => {
                    // Playout is pre-armed at the negotiated sequence, so a
                    // playout before the first frame arrives conceals via PLC;
                    // reaching NotReady here means the wiring lost the start sequence.
                    return Err(ScenarioError::InternalCapacityInvariant);
                }
            }
            playout_decisions += 1;
            next_playout_ms = next_playout_ms.saturating_add(config.frame_duration_ms);
        }
        clock.advance_by(1).map_err(ScenarioError::Clock)?;
    }

    report.network = network.stats();
    Ok(report)
}

fn validate(config: ScenarioConfig) -> Result<OpusFrameConfig, ScenarioError> {
    if config.frames == 0 || config.frames > MAX_SCENARIO_FRAMES {
        return Err(ScenarioError::InvalidFrameCount);
    }
    let frame_duration_ms =
        u16::try_from(config.frame_duration_ms).map_err(|_| ScenarioError::InvalidFrameDuration)?;
    let frame = OpusFrameConfig::new(OPUS_SAMPLE_RATE_HZ, OPUS_CHANNELS, frame_duration_ms)
        .map_err(|_| ScenarioError::InvalidFrameDuration)?;
    if config.target_buffer_ms > 1_000 {
        return Err(ScenarioError::InvalidTargetBuffer);
    }
    if config.media_datagram_bytes == 0
        || usize::from(config.media_datagram_bytes) > MAX_MEDIA_DATAGRAM_LEN
    {
        return Err(ScenarioError::InvalidMediaDatagramBytes);
    }
    config.network.validate().map_err(ScenarioError::Profile)?;
    Ok(frame)
}

fn sample_timestamp(sequence: u32, frame: OpusFrameConfig) -> u64 {
    u64::from(sequence).saturating_mul(frame.samples_per_channel() as u64)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn simulation_is_deterministic_and_accounts_every_playout() {
        let mut network = NetworkProfile::stable(42);
        network.loss_percent = 3;
        network.duplicate_percent = 5;
        network.reorder_percent = 10;
        let config = ScenarioConfig {
            network,
            ..ScenarioConfig::default()
        };

        let first = simulate(config).unwrap();
        let second = simulate(config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.playout_frames + first.plc_frames, config.frames);
        assert_eq!(first.resynchronizations, 0);
        assert!(first.plc_frames > 0);
    }

    #[test]
    fn burst_loss_is_visible_as_plc() {
        let mut network = NetworkProfile::stable(7);
        network.burst_interval_packets = 20;
        network.burst_length_packets = 3;
        let config = ScenarioConfig {
            frames: 100,
            network,
            ..ScenarioConfig::default()
        };

        let report = simulate(config).unwrap();
        assert_eq!(report.network.burst_loss_packets, 12);
        assert_eq!(report.plc_frames, 12);
    }

    #[test]
    fn finite_bandwidth_applies_the_configured_datagram_size() {
        let mut network = NetworkProfile::stable(9);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        network.bandwidth_bytes_per_second = 1_000;
        let config = ScenarioConfig {
            frames: 5,
            target_buffer_ms: 60,
            media_datagram_bytes: 100,
            network,
            ..ScenarioConfig::default()
        };

        let report = simulate(config).unwrap();

        assert_eq!(report.network.bandwidth_shaped_copies, 5);
        assert_eq!(report.network.bandwidth_serialized_bytes, 500);
        assert!(report.plc_frames > 0);
        assert_eq!(report.playout_frames + report.plc_frames, config.frames);
    }

    #[test]
    fn rejects_media_datagrams_outside_the_protocol_bound() {
        let zero = ScenarioConfig {
            media_datagram_bytes: 0,
            ..ScenarioConfig::default()
        };
        assert_eq!(
            simulate(zero),
            Err(ScenarioError::InvalidMediaDatagramBytes)
        );

        let too_large = ScenarioConfig {
            media_datagram_bytes: u16::try_from(MAX_MEDIA_DATAGRAM_LEN + 1).unwrap(),
            ..ScenarioConfig::default()
        };
        assert_eq!(
            simulate(too_large),
            Err(ScenarioError::InvalidMediaDatagramBytes)
        );
    }

    #[test]
    fn negotiated_frame_duration_controls_48khz_sample_timestamps() {
        let ten_ms = OpusFrameConfig::new(48_000, 2, 10).unwrap();
        let twenty_ms = OpusFrameConfig::new(48_000, 2, 20).unwrap();

        assert_eq!(sample_timestamp(3, ten_ms), 1_440);
        assert_eq!(sample_timestamp(3, twenty_ms), 2_880);
    }

    #[test]
    fn both_opus_frame_shapes_remain_lossless_on_a_stable_profile() {
        let mut network = NetworkProfile::stable(0x0A0C_2026);
        network.base_delay_ms = 0;
        network.jitter_ms = 0;
        let ten_ms = ScenarioConfig {
            frames: 7,
            frame_duration_ms: 10,
            target_buffer_ms: 10,
            network,
            ..ScenarioConfig::default()
        };
        let twenty_ms = ScenarioConfig {
            frame_duration_ms: 20,
            target_buffer_ms: 20,
            ..ten_ms
        };

        for config in [ten_ms, twenty_ms] {
            let report = simulate(config).unwrap();
            assert_eq!(report.playout_frames, config.frames);
            assert_eq!(report.plc_frames, 0);
        }
    }

    #[test]
    fn send_buffer_backpressure_is_accounted_and_concealed() {
        // A finite link with a sender-side socket buffer that admits every
        // other 256-byte datagram: rejected frames are counted exactly once
        // and concealed by PLC while admitted frames still decode in sequence.
        let mut network = NetworkProfile::stable(0x51DB_2026);
        network.loss_percent = 0;
        network.duplicate_percent = 0;
        network.reorder_percent = 0;
        network.base_delay_ms = 1;
        network.jitter_ms = 0;
        network.reorder_delay_ms = 0;
        network.bandwidth_bytes_per_second = 6_400; // one datagram per 40 ms
        network.send_buffer_bytes = Some(256);
        let config = ScenarioConfig {
            frames: 200,
            media_datagram_bytes: 256,
            network,
            ..ScenarioConfig::default()
        };

        let first = simulate(config).unwrap();
        let second = simulate(config).unwrap();

        assert_eq!(first, second, "scenario stays deterministic");
        assert!(
            first.network.send_buffer_full_packets > 0,
            "bounded sender buffer must report backpressure"
        );
        assert_eq!(
            first.playout_frames + first.plc_frames,
            config.frames,
            "every playout slot renders decoded audio or PLC"
        );
        assert!(
            first.plc_frames > 0,
            "rejected frames are concealed with PLC"
        );
        assert_eq!(first.network.random_loss_packets, 0);
        assert_eq!(first.network.burst_loss_packets, 0);
        // No loss, duplication, or capacity drops: every submission is either
        // admitted or rejected by the sender-side socket buffer.
        assert_eq!(
            first.network.send_buffer_full_packets + first.network.scheduled_copies,
            first.network.submitted_packets,
            "send-buffer backpressure conserves every submission"
        );
    }

    #[test]
    fn jitter_reorder_and_duplicate_delivery_stays_accounted_at_scenario_level() {
        // A bounded jitter and reorder profile with duplicate delivery: the
        // target buffer absorbs every arrival, so no PLC or hard
        // resynchronization occurs, every network duplicate copy is rejected by
        // jitter storage, and reordered arrivals stay within the network count.
        let mut network = NetworkProfile::stable(0x4A32_2026);
        network.loss_percent = 0;
        network.base_delay_ms = 5;
        network.jitter_ms = 15;
        network.duplicate_percent = 12;
        network.reorder_percent = 25;
        network.reorder_delay_ms = 10;
        let config = ScenarioConfig {
            frames: 300,
            target_buffer_ms: 80,
            network,
            ..ScenarioConfig::default()
        };

        let first = simulate(config).unwrap();
        let second = simulate(config).unwrap();

        assert_eq!(first, second, "scenario stays deterministic");
        assert_eq!(
            first.playout_frames + first.plc_frames,
            config.frames,
            "every playout slot renders decoded audio or PLC"
        );
        assert_eq!(
            first.plc_frames, 0,
            "the target buffer absorbs jitter, reorder, and duplicate delivery"
        );
        assert_eq!(
            first.resynchronizations, 0,
            "bounded jitter and reorder never force a hard reset"
        );
        assert!(
            first.duplicate_frames > 0,
            "duplicate delivery must be rejected by jitter storage"
        );
        assert!(
            first.reordered_arrivals > 0,
            "reorder delay must perturb arrival order"
        );
        // Every duplicate copy arrives while its original is still buffered
        // (worst-case delay is below the target buffer), so jitter storage
        // rejects it as a duplicate instead of admitting it as a new frame.
        assert_eq!(
            u64::from(first.duplicate_frames),
            first.network.duplicate_copies,
            "every network duplicate copy is rejected by jitter storage"
        );
        // A reordered arrival comes only from a packet that received the
        // configured reorder delay; not every delayed packet is inserted out
        // of order, so the jitter counter is a subset of the network counter.
        assert!(
            u64::from(first.reordered_arrivals) <= first.network.reordered_packets,
            "jitter reordered arrivals are a subset of network reordered packets"
        );
    }
    #[test]
    fn rejects_unnegotiated_frame_duration() {
        let config = ScenarioConfig {
            frame_duration_ms: 15,
            ..ScenarioConfig::default()
        };

        assert_eq!(simulate(config), Err(ScenarioError::InvalidFrameDuration));
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let profile = ScenarioError::Profile(ProfileError::DelayTooLarge);
        assert!(
            Error::source(&profile)
                .and_then(|source| source.downcast_ref::<ProfileError>())
                .is_some()
        );

        let clock = ScenarioError::Clock(ClockError::Overflow);
        assert!(
            Error::source(&clock)
                .and_then(|source| source.downcast_ref::<ClockError>())
                .is_some()
        );

        assert!(Error::source(&ScenarioError::InvalidFrameCount).is_none());
        assert!(Error::source(&ScenarioError::InternalCapacityInvariant).is_none());
    }
}
