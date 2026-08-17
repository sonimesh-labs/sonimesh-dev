//! Bounded target-gated playout scheduling primitive (ADR 0016).
//!
//! The [`AdaptiveJitterTarget`] controller selects a water level; this type
//! turns that water level into one per-tick playout decision. It waits while
//! fewer than the target encoded frames are buffered and arrivals are still
//! pending, plays as soon as the target is met or no further frames are in
//! flight, and forces a playout when the gate deadline expires so a mid-stream
//! stall stays bounded. It is a scheduling-domain primitive: callers must
//! never run it from an output callback, and it observes only the
//! encoded-frame water level and the in-flight signal, never PCM queue
//! occupancy ([ADR 0007](../../docs/adr/0007-drift-control-decision.md) keeps
//! that signal with the drift controller).

use core::fmt;

/// Hard upper bound on consecutive gated playout ticks before a forced
/// playout.
pub const MAX_GATE_TICKS: u32 = 64;

/// One per-tick target-gated playout decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayoutDecision {
    /// Play the next frame, or conceal a missing one, this tick.
    Play,
    /// Wait one tick: fewer than the target frames are buffered and arrivals
    /// are still pending inside the gate deadline.
    Gate,
}

/// Configuration for one bounded target-gated playout scheduler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetGatedPlayoutConfig {
    /// Consecutive gated ticks allowed before the scheduler plays out anyway.
    pub max_gate_ticks: u32,
}

impl TargetGatedPlayoutConfig {
    /// Validates the gate deadline against the hard bound.
    ///
    /// # Errors
    ///
    /// Returns [`PlayoutSchedulerConfigError::ZeroMaxGateTicks`] for a zero
    /// deadline and [`PlayoutSchedulerConfigError::MaxGateTicksTooLarge`]
    /// when the deadline exceeds [`MAX_GATE_TICKS`].
    pub const fn validate(self) -> Result<(), PlayoutSchedulerConfigError> {
        if self.max_gate_ticks == 0 {
            return Err(PlayoutSchedulerConfigError::ZeroMaxGateTicks);
        }
        if self.max_gate_ticks > MAX_GATE_TICKS {
            return Err(PlayoutSchedulerConfigError::MaxGateTicksTooLarge);
        }
        Ok(())
    }
}

/// Invalid target-gated playout configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayoutSchedulerConfigError {
    /// The gate deadline must be nonzero.
    ZeroMaxGateTicks,
    /// The gate deadline exceeds [`MAX_GATE_TICKS`].
    MaxGateTicksTooLarge,
}

impl fmt::Display for PlayoutSchedulerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMaxGateTicks => formatter.write_str("gate deadline must be nonzero"),
            Self::MaxGateTicksTooLarge => {
                write!(
                    formatter,
                    "gate deadline exceeds the {MAX_GATE_TICKS}-tick hard bound"
                )
            }
        }
    }
}

impl std::error::Error for PlayoutSchedulerConfigError {}

/// Bounded target-gated playout scheduler for one stream.
///
/// The scheduler owns the gate history and the bounded counters. Call
/// [`Self::decide`] once per scheduling tick with the buffered encoded-frame
/// count, the adaptive target water level, and whether further arrivals are
/// still in flight; the returned decision advances the gate state. All
/// arithmetic is saturating, so a long or adversarial run cannot wrap the
/// counters.
#[derive(Clone, Copy, Debug)]
pub struct TargetGatedPlayout {
    config: TargetGatedPlayoutConfig,
    gated_in_a_row: u32,
    gated_ticks: u64,
    forced_playouts: u64,
}

impl TargetGatedPlayout {
    /// Creates a scheduler with the given gate deadline.
    ///
    /// # Errors
    ///
    /// Returns [`PlayoutSchedulerConfigError`] when the gate deadline is
    /// zero or exceeds [`MAX_GATE_TICKS`].
    pub fn new(config: TargetGatedPlayoutConfig) -> Result<Self, PlayoutSchedulerConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            gated_in_a_row: 0,
            gated_ticks: 0,
            forced_playouts: 0,
        })
    }

    /// Returns the configured gate deadline in ticks.
    #[must_use]
    pub const fn max_gate_ticks(&self) -> u32 {
        self.config.max_gate_ticks
    }

    /// Returns the current consecutive gated-tick streak.
    #[must_use]
    pub const fn gated_in_a_row(&self) -> u32 {
        self.gated_in_a_row
    }

    /// Returns the total ticks waited below the target with arrivals pending.
    #[must_use]
    pub const fn gated_ticks(&self) -> u64 {
        self.gated_ticks
    }

    /// Returns the total plays forced by the gate deadline.
    #[must_use]
    pub const fn forced_playouts(&self) -> u64 {
        self.forced_playouts
    }

    /// Makes one bounded playout decision and advances the gate state.
    ///
    /// Playing below the target with arrivals still pending is only reachable
    /// through an expired gate deadline, so that play counts as forced.
    /// Playing at or above the target, or when no arrivals are pending, never
    /// counts as forced.
    #[must_use]
    pub fn decide(
        &mut self,
        buffered_frames: usize,
        target_frames: usize,
        arrivals_pending: bool,
    ) -> PlayoutDecision {
        let below_target = buffered_frames < target_frames;
        let gate_expired = self.gated_in_a_row >= self.config.max_gate_ticks;
        if !arrivals_pending || !below_target || gate_expired {
            if below_target && arrivals_pending {
                self.forced_playouts = self.forced_playouts.saturating_add(1);
            }
            self.gated_in_a_row = 0;
            PlayoutDecision::Play
        } else {
            self.gated_in_a_row = self.gated_in_a_row.saturating_add(1);
            self.gated_ticks = self.gated_ticks.saturating_add(1);
            PlayoutDecision::Gate
        }
    }

    /// Resets the gate history and counters for a fresh stream epoch.
    ///
    /// A stream reset must not inherit a stale gate streak or cumulative
    /// counters from the retired epoch.
    pub fn reset(&mut self) {
        self.gated_in_a_row = 0;
        self.gated_ticks = 0;
        self.forced_playouts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_GATE_TICKS, PlayoutDecision, PlayoutSchedulerConfigError, TargetGatedPlayout,
        TargetGatedPlayoutConfig,
    };

    #[test]
    fn config_rejects_zero_and_oversized_deadlines() {
        assert_eq!(
            TargetGatedPlayoutConfig { max_gate_ticks: 0 }
                .validate()
                .unwrap_err(),
            PlayoutSchedulerConfigError::ZeroMaxGateTicks
        );
        assert_eq!(
            TargetGatedPlayoutConfig {
                max_gate_ticks: MAX_GATE_TICKS + 1
            }
            .validate()
            .unwrap_err(),
            PlayoutSchedulerConfigError::MaxGateTicksTooLarge
        );
        assert!(
            TargetGatedPlayoutConfig { max_gate_ticks: 1 }
                .validate()
                .is_ok()
        );
        assert!(
            TargetGatedPlayoutConfig {
                max_gate_ticks: MAX_GATE_TICKS
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn gates_while_below_target_with_pending_arrivals() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 8 }).unwrap();

        assert_eq!(scheduler.decide(2, 4, true), PlayoutDecision::Gate);
        assert_eq!(scheduler.gated_in_a_row(), 1);
        assert_eq!(scheduler.gated_ticks(), 1);
        assert_eq!(scheduler.forced_playouts(), 0);
    }

    #[test]
    fn plays_when_target_is_met_even_with_pending_arrivals() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 8 }).unwrap();

        assert_eq!(scheduler.decide(4, 4, true), PlayoutDecision::Play);
        assert_eq!(scheduler.forced_playouts(), 0);
        assert_eq!(scheduler.gated_in_a_row(), 0);
    }

    #[test]
    fn drains_without_gating_once_no_arrivals_are_pending() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 8 }).unwrap();

        assert_eq!(scheduler.decide(0, 4, false), PlayoutDecision::Play);
        assert_eq!(scheduler.forced_playouts(), 0, "drain is never forced");
    }

    #[test]
    fn forces_playout_when_the_gate_deadline_expires() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 2 }).unwrap();

        assert_eq!(scheduler.decide(0, 4, true), PlayoutDecision::Gate);
        assert_eq!(scheduler.decide(0, 4, true), PlayoutDecision::Gate);
        assert_eq!(scheduler.gated_in_a_row(), 2);
        assert_eq!(
            scheduler.decide(0, 4, true),
            PlayoutDecision::Play,
            "the third tick past the deadline must force playout"
        );
        assert_eq!(scheduler.forced_playouts(), 1);
        assert_eq!(scheduler.gated_in_a_row(), 0);
        assert_eq!(scheduler.gated_ticks(), 2);
    }

    #[test]
    fn never_gates_more_than_the_configured_deadline_in_a_row() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 3 }).unwrap();

        for tick in 1..=3 {
            assert_eq!(scheduler.decide(1, 4, true), PlayoutDecision::Gate);
            assert_eq!(
                scheduler.gated_in_a_row(),
                tick,
                "streak grows by one per gated tick"
            );
        }
        assert_eq!(scheduler.decide(1, 4, true), PlayoutDecision::Play);
        assert_eq!(scheduler.forced_playouts(), 1);
    }

    #[test]
    fn target_met_after_gating_resets_the_streak_without_force() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 8 }).unwrap();
        let _ = scheduler.decide(1, 5, true);
        let _ = scheduler.decide(2, 5, true);

        assert_eq!(scheduler.decide(5, 5, true), PlayoutDecision::Play);
        assert_eq!(scheduler.forced_playouts(), 0);
        assert_eq!(scheduler.gated_in_a_row(), 0);
        assert_eq!(scheduler.gated_ticks(), 2);
    }

    #[test]
    fn reset_clears_history_and_counters() {
        let mut scheduler =
            TargetGatedPlayout::new(TargetGatedPlayoutConfig { max_gate_ticks: 1 }).unwrap();
        let _ = scheduler.decide(0, 4, true);
        let _ = scheduler.decide(0, 4, true);

        scheduler.reset();

        assert_eq!(scheduler.gated_in_a_row(), 0);
        assert_eq!(scheduler.gated_ticks(), 0);
        assert_eq!(scheduler.forced_playouts(), 0);
        assert_eq!(scheduler.decide(0, 4, true), PlayoutDecision::Gate);
    }
}
