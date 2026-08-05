use core::fmt;

use crate::MAX_PCM_QUEUE_BLOCKS;

/// Largest absolute ASRC ratio adjustment the controller can request.
pub const MAX_DRIFT_ADJUSTMENT_PPM: i32 = 2_000;
/// Largest absolute accumulated occupancy error in block-tick units.
pub const MAX_DRIFT_INTEGRAL_BLOCK_TICKS: i32 = 32_768;

/// Bounded occupancy PI-controller configuration for one stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriftConfig {
    /// Desired count of decoded PCM blocks in the realtime queue.
    pub target_blocks: u16,
    /// Occupancy at or below this value requests a low-side resynchronization.
    pub low_resync_blocks: u16,
    /// Occupancy at or above this value requests a high-side resynchronization.
    pub high_resync_blocks: u16,
    /// Instantaneous correction contributed by one occupancy-block error.
    pub proportional_ppm_per_block: i32,
    /// Per-update correction contributed by one accumulated block-tick error.
    pub integral_ppm_per_block_tick: i32,
    /// Absolute cap on the requested ratio adjustment.
    pub max_adjustment_ppm: i32,
    /// Absolute cap on adjustment change between consecutive updates.
    pub max_step_ppm: i32,
    /// Absolute cap on accumulated occupancy error.
    pub max_integral_block_ticks: i32,
}

impl DriftConfig {
    /// Validates target, thresholds, and all controller bounds.
    ///
    /// # Errors
    ///
    /// Returns [`DriftConfigError`] when thresholds do not protect the target,
    /// gains are non-positive, or a requested correction could exceed a hard
    /// controller bound.
    pub fn validate(self) -> Result<(), DriftConfigError> {
        if self.low_resync_blocks >= self.target_blocks
            || self.target_blocks >= self.high_resync_blocks
            || usize::from(self.high_resync_blocks) > MAX_PCM_QUEUE_BLOCKS
        {
            return Err(DriftConfigError::InvalidOccupancyThresholds);
        }
        if self.proportional_ppm_per_block <= 0 || self.integral_ppm_per_block_tick <= 0 {
            return Err(DriftConfigError::InvalidGain);
        }
        if self.max_adjustment_ppm <= 0 || self.max_adjustment_ppm > MAX_DRIFT_ADJUSTMENT_PPM {
            return Err(DriftConfigError::InvalidAdjustmentLimit);
        }
        if self.max_step_ppm <= 0 || self.max_step_ppm > self.max_adjustment_ppm {
            return Err(DriftConfigError::InvalidStepLimit);
        }
        if self.max_integral_block_ticks <= 0
            || self.max_integral_block_ticks > MAX_DRIFT_INTEGRAL_BLOCK_TICKS
        {
            return Err(DriftConfigError::InvalidIntegralLimit);
        }
        Ok(())
    }
}

/// Invalid bounded drift-controller configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftConfigError {
    /// Target is not strictly between the two emergency occupancy thresholds.
    InvalidOccupancyThresholds,
    /// A proportional or integral gain is zero or negative.
    InvalidGain,
    /// Ratio cap is zero, negative, or exceeds the controller hard limit.
    InvalidAdjustmentLimit,
    /// Update slew cap is zero, negative, or exceeds the ratio cap.
    InvalidStepLimit,
    /// Integral accumulator cap is zero, negative, or exceeds its hard limit.
    InvalidIntegralLimit,
}

impl fmt::Display for DriftConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid drift controller configuration: {self:?}"
        )
    }
}

impl std::error::Error for DriftConfigError {}

/// Side of the PCM occupancy range that forced a stream-local reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftResyncDirection {
    /// PCM occupancy reached the low emergency threshold.
    Low,
    /// PCM occupancy reached the high emergency threshold.
    High,
}

/// One bounded control decision for a future ASRC or stream recovery owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftDecision {
    /// Consume PCM at the nominal rate plus this signed ppm adjustment.
    Adjust {
        /// Positive values consume faster because occupancy is above target.
        ppm: i32,
    },
    /// Reset dependent stream-local state before normal adjustment resumes.
    HardResync {
        /// Emergency occupancy boundary that was reached.
        direction: DriftResyncDirection,
    },
}

/// Non-realtime bounded PI controller for one stream's PCM queue occupancy.
pub struct DriftController {
    config: DriftConfig,
    integral_block_ticks: i32,
    adjustment_ppm: i32,
}

impl DriftController {
    /// Creates a controller with zero history and nominal-rate output.
    ///
    /// # Errors
    ///
    /// Returns [`DriftConfigError`] when a controller parameter is unbounded or
    /// cannot preserve an emergency margin around the target.
    pub fn new(config: DriftConfig) -> Result<Self, DriftConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            integral_block_ticks: 0,
            adjustment_ppm: 0,
        })
    }

    /// Returns the current bounded ppm adjustment for diagnostics.
    #[must_use]
    pub const fn adjustment_ppm(&self) -> i32 {
        self.adjustment_ppm
    }

    /// Clears stream-local control history after an external reset.
    pub fn reset(&mut self) {
        self.integral_block_ticks = 0;
        self.adjustment_ppm = 0;
    }

    /// Converts one PCM occupancy snapshot into a bounded rate or reset decision.
    #[must_use]
    pub fn update(&mut self, occupancy_blocks: u16) -> DriftDecision {
        if occupancy_blocks <= self.config.low_resync_blocks {
            self.reset();
            return DriftDecision::HardResync {
                direction: DriftResyncDirection::Low,
            };
        }
        if occupancy_blocks >= self.config.high_resync_blocks {
            self.reset();
            return DriftDecision::HardResync {
                direction: DriftResyncDirection::High,
            };
        }

        let error_blocks = i32::from(occupancy_blocks) - i32::from(self.config.target_blocks);
        let candidate_integral = (self.integral_block_ticks + error_blocks).clamp(
            -self.config.max_integral_block_ticks,
            self.config.max_integral_block_ticks,
        );
        let candidate_target = self.target_adjustment(error_blocks, candidate_integral);
        let clamped_candidate = candidate_target.clamp(
            -self.config.max_adjustment_ppm,
            self.config.max_adjustment_ppm,
        );
        let drives_farther_into_clamp = (candidate_target > clamped_candidate && error_blocks > 0)
            || (candidate_target < clamped_candidate && error_blocks < 0);
        if !drives_farther_into_clamp {
            self.integral_block_ticks = candidate_integral;
        }

        let target = self
            .target_adjustment(error_blocks, self.integral_block_ticks)
            .clamp(
                -self.config.max_adjustment_ppm,
                self.config.max_adjustment_ppm,
            );
        let delta = (target - self.adjustment_ppm)
            .clamp(-self.config.max_step_ppm, self.config.max_step_ppm);
        self.adjustment_ppm += delta;
        DriftDecision::Adjust {
            ppm: self.adjustment_ppm,
        }
    }

    fn target_adjustment(&self, error_blocks: i32, integral_block_ticks: i32) -> i32 {
        let proportional =
            i64::from(error_blocks) * i64::from(self.config.proportional_ppm_per_block);
        let integral =
            i64::from(integral_block_ticks) * i64::from(self.config.integral_ppm_per_block_tick);
        let bounded = (proportional + integral).clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        i32::try_from(bounded).unwrap_or(if bounded.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: DriftConfig = DriftConfig {
        target_blocks: 4,
        low_resync_blocks: 0,
        high_resync_blocks: 8,
        proportional_ppm_per_block: 100,
        integral_ppm_per_block_tick: 10,
        max_adjustment_ppm: 500,
        max_step_ppm: 50,
        max_integral_block_ticks: 16,
    };

    #[test]
    fn rejects_threshold_and_bound_violations() {
        let mut config = CONFIG;
        config.low_resync_blocks = config.target_blocks;
        assert!(matches!(
            DriftController::new(config),
            Err(DriftConfigError::InvalidOccupancyThresholds)
        ));

        config = CONFIG;
        config.max_step_ppm = config.max_adjustment_ppm + 1;
        assert!(matches!(
            DriftController::new(config),
            Err(DriftConfigError::InvalidStepLimit)
        ));
    }

    #[test]
    fn positive_occupancy_error_consumes_faster_with_slew_limit() {
        let mut controller = DriftController::new(CONFIG).unwrap();

        assert_eq!(controller.update(6), DriftDecision::Adjust { ppm: 50 });
        assert_eq!(controller.update(6), DriftDecision::Adjust { ppm: 100 });
    }

    #[test]
    fn negative_occupancy_error_consumes_slower_with_slew_limit() {
        let mut controller = DriftController::new(CONFIG).unwrap();

        assert_eq!(controller.update(2), DriftDecision::Adjust { ppm: -50 });
        assert_eq!(controller.update(2), DriftDecision::Adjust { ppm: -100 });
    }

    #[test]
    fn clamp_does_not_wind_up_integral_against_persistent_error() {
        let config = DriftConfig {
            max_adjustment_ppm: 100,
            max_step_ppm: 100,
            integral_ppm_per_block_tick: 100,
            ..CONFIG
        };
        let mut controller = DriftController::new(config).unwrap();

        assert_eq!(controller.update(7), DriftDecision::Adjust { ppm: 100 });
        assert_eq!(controller.integral_block_ticks, 0);
        assert_eq!(controller.update(4), DriftDecision::Adjust { ppm: 0 });
    }

    #[test]
    fn hard_resync_clears_adjustment_and_integral_history_in_both_directions() {
        let mut high_side_controller = DriftController::new(CONFIG).unwrap();
        assert_eq!(
            high_side_controller.update(6),
            DriftDecision::Adjust { ppm: 50 }
        );
        assert_eq!(high_side_controller.integral_block_ticks, 2);
        assert_eq!(
            high_side_controller.update(8),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::High,
            }
        );
        assert_eq!(high_side_controller.adjustment_ppm(), 0);
        assert_eq!(high_side_controller.integral_block_ticks, 0);
        assert_eq!(
            high_side_controller.update(4),
            DriftDecision::Adjust { ppm: 0 }
        );

        let mut low_side_controller = DriftController::new(CONFIG).unwrap();
        assert_eq!(
            low_side_controller.update(2),
            DriftDecision::Adjust { ppm: -50 }
        );
        assert_eq!(low_side_controller.integral_block_ticks, -2);
        assert_eq!(
            low_side_controller.update(0),
            DriftDecision::HardResync {
                direction: DriftResyncDirection::Low,
            }
        );
        assert_eq!(low_side_controller.adjustment_ppm(), 0);
        assert_eq!(low_side_controller.integral_block_ticks, 0);
        assert_eq!(
            low_side_controller.update(4),
            DriftDecision::Adjust { ppm: 0 }
        );
    }
}
