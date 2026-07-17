use std::fmt;

use sonimesh_audio::{EncodedFrame, InsertOutcome, JitterBuffer, PlayoutOutcome};

use crate::{ClockError, NetworkProfile, NetworkStats, ProfileError, VirtualClock, VirtualNetwork};

const MAX_SCENARIO_FRAMES: u32 = 100_000;

/// One deterministic encoded-audio simulation configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScenarioConfig {
    /// Number of sender frames.
    pub frames: u32,
    /// Frame duration used by sender and receiver scheduling.
    pub frame_duration_ms: u64,
    /// Fixed initial playout delay.
    pub target_buffer_ms: u64,
    /// Virtual network impairments.
    pub network: NetworkProfile,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            frames: 500,
            frame_duration_ms: 20,
            target_buffer_ms: 60,
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
    /// Frame duration must be between 1 and 120 milliseconds.
    InvalidFrameDuration,
    /// Initial playout target must be no more than one second.
    InvalidTargetBuffer,
    /// Network profile is invalid.
    Profile(ProfileError),
    /// Virtual time overflowed.
    Clock(ClockError),
    /// A compile-time simulation capacity contradicted its runtime use.
    InternalCapacityInvariant,
}

impl fmt::Display for ScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid deterministic scenario: {self:?}")
    }
}

impl std::error::Error for ScenarioError {}

/// Runs a deterministic encoded-frame and jitter-buffer scenario.
///
/// # Errors
///
/// Returns [`ScenarioError`] when configuration or virtual time is invalid.
pub fn simulate(config: ScenarioConfig) -> Result<SimulationReport, ScenarioError> {
    validate(config)?;
    let mut network = VirtualNetwork::new(config.network).map_err(ScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let mut jitter = JitterBuffer::<64, 8>::with_start_sequence(0);
    let mut report = SimulationReport::default();
    let mut next_send_sequence = 0_u32;
    let mut next_send_ms = 0_u64;
    let mut next_playout_ms = config.target_buffer_ms;
    let mut playout_decisions = 0_u32;
    let mut highest_arrival = None;
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
            network.send(now_ms, next_send_sequence);
            next_send_sequence += 1;
            next_send_ms = next_send_ms.saturating_add(config.frame_duration_ms);
        }

        for delivery in network.drain_ready(now_ms) {
            let sequence = delivery.payload;
            if highest_arrival.is_some_and(|highest| sequence < highest) {
                report.reordered_arrivals += 1;
            }
            highest_arrival =
                Some(highest_arrival.map_or(sequence, |highest| highest.max(sequence)));
            let frame = EncodedFrame::new(sequence, u64::from(sequence) * 960, &[0])
                .map_err(|_| ScenarioError::InternalCapacityInvariant)?;
            match jitter.insert(frame) {
                InsertOutcome::Inserted => {}
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
                    return Err(ScenarioError::InvalidTargetBuffer);
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

fn validate(config: ScenarioConfig) -> Result<(), ScenarioError> {
    if config.frames == 0 || config.frames > MAX_SCENARIO_FRAMES {
        return Err(ScenarioError::InvalidFrameCount);
    }
    if !(1..=120).contains(&config.frame_duration_ms) {
        return Err(ScenarioError::InvalidFrameDuration);
    }
    if config.target_buffer_ms > 1_000 {
        return Err(ScenarioError::InvalidTargetBuffer);
    }
    config.network.validate().map_err(ScenarioError::Profile)
}

#[cfg(test)]
mod tests {
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
}
