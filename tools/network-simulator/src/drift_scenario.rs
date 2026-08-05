use core::fmt;

use sonimesh_audio::{
    DriftConfig, DriftConfigError, DriftController, DriftDecision, DriftResyncDirection,
    MAX_PCM_QUEUE_BLOCKS,
};

const MAX_DRIFT_SCENARIO_TICKS: u32 = 100_000;
const MAX_SOURCE_RATE_PPM: i32 = 10_000;
const BLOCK_QUANTA: i64 = 1_000_000;

/// Deterministic integer occupancy model for one future ASRC control loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriftScenarioConfig {
    /// Number of fixed-duration PCM block ticks to simulate.
    pub ticks: u32,
    /// Initial occupancy snapshot supplied to the controller.
    pub initial_occupancy_blocks: u16,
    /// Sender clock offset from nominal in ppm.
    pub source_rate_ppm: i32,
    /// Bounded per-stream controller configuration.
    pub controller: DriftConfig,
}

/// Deterministic closed-loop result for a synthetic occupancy plant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriftSimulationReport {
    /// Smallest rounded PCM occupancy observed before a resynchronization.
    pub minimum_occupancy_blocks: u16,
    /// Largest rounded PCM occupancy observed before a resynchronization.
    pub maximum_occupancy_blocks: u16,
    /// Occupancy at the final simulated tick.
    pub final_occupancy_blocks: u16,
    /// Final controller rate request in ppm.
    pub final_adjustment_ppm: i32,
    /// Number of hard-resynchronization requests applied by the model owner.
    pub hard_resynchronizations: u32,
    /// Low-side reset requests.
    pub low_resynchronizations: u32,
    /// High-side reset requests.
    pub high_resynchronizations: u32,
}

/// Invalid deterministic drift scenario configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriftScenarioError {
    /// Tick count is zero or exceeds the simulation hard bound.
    InvalidTickCount,
    /// Initial occupancy cannot fit the bounded PCM queue model.
    InvalidInitialOccupancy,
    /// Synthetic sender offset exceeds the model hard bound.
    InvalidSourceRate,
    /// Per-stream controller configuration is invalid.
    Controller(DriftConfigError),
}

impl fmt::Display for DriftScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTickCount => formatter
                .write_str("invalid drift scenario: tick count is outside the explicit bound"),
            Self::InvalidInitialOccupancy => formatter.write_str(
                "invalid drift scenario: initial occupancy exceeds the bounded PCM queue",
            ),
            Self::InvalidSourceRate => formatter.write_str(
                "invalid drift scenario: source rate is outside the explicit model bound",
            ),
            Self::Controller(error) => {
                write!(formatter, "invalid drift scenario controller: {error}")
            }
        }
    }
}

impl std::error::Error for DriftScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Controller(error) => Some(error),
            Self::InvalidTickCount | Self::InvalidInitialOccupancy | Self::InvalidSourceRate => {
                None
            }
        }
    }
}

/// Runs an integerized PCM-occupancy control loop without an ASRC implementation.
///
/// One tick produces and consumes one nominal PCM block. Only the difference
/// between `source_rate_ppm` and the controller decision changes occupancy.
/// When the controller requests a hard resynchronization, this model resets the
/// synthetic occupancy to target; production stream code owns the actual flush,
/// fade, and ASRC reset work.
///
/// # Errors
///
/// Returns [`DriftScenarioError`] when the synthetic tick, occupancy, source
/// rate, or controller configuration is outside its explicit bound.
pub fn simulate_drift(
    config: DriftScenarioConfig,
) -> Result<DriftSimulationReport, DriftScenarioError> {
    validate(config)?;
    let mut controller =
        DriftController::new(config.controller).map_err(DriftScenarioError::Controller)?;
    let mut occupancy_quanta = i64::from(config.initial_occupancy_blocks) * BLOCK_QUANTA;
    let mut report = DriftSimulationReport {
        minimum_occupancy_blocks: config.initial_occupancy_blocks,
        maximum_occupancy_blocks: config.initial_occupancy_blocks,
        final_occupancy_blocks: config.initial_occupancy_blocks,
        final_adjustment_ppm: 0,
        hard_resynchronizations: 0,
        low_resynchronizations: 0,
        high_resynchronizations: 0,
    };

    for _ in 0..config.ticks {
        let occupancy = rounded_occupancy(occupancy_quanta);
        report.minimum_occupancy_blocks = report.minimum_occupancy_blocks.min(occupancy);
        report.maximum_occupancy_blocks = report.maximum_occupancy_blocks.max(occupancy);
        let adjustment_ppm = match controller.update(occupancy) {
            DriftDecision::Adjust { ppm } => ppm,
            DriftDecision::HardResync { direction } => {
                report.hard_resynchronizations += 1;
                match direction {
                    DriftResyncDirection::Low => report.low_resynchronizations += 1,
                    DriftResyncDirection::High => report.high_resynchronizations += 1,
                }
                occupancy_quanta = i64::from(config.controller.target_blocks) * BLOCK_QUANTA;
                continue;
            }
        };
        occupancy_quanta = occupancy_quanta
            .saturating_add(i64::from(config.source_rate_ppm) - i64::from(adjustment_ppm));
    }

    report.final_occupancy_blocks = rounded_occupancy(occupancy_quanta);
    report.final_adjustment_ppm = controller.adjustment_ppm();
    Ok(report)
}

fn validate(config: DriftScenarioConfig) -> Result<(), DriftScenarioError> {
    if config.ticks == 0 || config.ticks > MAX_DRIFT_SCENARIO_TICKS {
        return Err(DriftScenarioError::InvalidTickCount);
    }
    if usize::from(config.initial_occupancy_blocks) > MAX_PCM_QUEUE_BLOCKS {
        return Err(DriftScenarioError::InvalidInitialOccupancy);
    }
    if i64::from(config.source_rate_ppm).abs() > i64::from(MAX_SOURCE_RATE_PPM) {
        return Err(DriftScenarioError::InvalidSourceRate);
    }
    config
        .controller
        .validate()
        .map_err(DriftScenarioError::Controller)
}

fn rounded_occupancy(quanta: i64) -> u16 {
    let maximum = i64::try_from(MAX_PCM_QUEUE_BLOCKS).unwrap_or(i64::MAX) * BLOCK_QUANTA;
    let bounded = quanta.clamp(0, maximum);
    let rounded = (bounded + BLOCK_QUANTA / 2) / BLOCK_QUANTA;
    u16::try_from(rounded).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use sonimesh_audio::MAX_DRIFT_ADJUSTMENT_PPM;

    const CONTROLLER: DriftConfig = DriftConfig {
        target_blocks: 4,
        low_resync_blocks: 0,
        high_resync_blocks: 8,
        proportional_ppm_per_block: 25,
        integral_ppm_per_block_tick: 1,
        max_adjustment_ppm: 500,
        max_step_ppm: 25,
        max_integral_block_ticks: 256,
    };

    #[test]
    fn closed_loop_is_deterministic_and_bounded_for_correctable_drift() {
        let config = DriftScenarioConfig {
            ticks: 20_000,
            initial_occupancy_blocks: 5,
            source_rate_ppm: 100,
            controller: CONTROLLER,
        };

        let first = simulate_drift(config).unwrap();
        let second = simulate_drift(config).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.hard_resynchronizations, 0);
        assert!((2..=7).contains(&first.final_occupancy_blocks));
        assert!(first.final_adjustment_ppm.abs() <= MAX_DRIFT_ADJUSTMENT_PPM);
    }

    #[test]
    fn uncorrectable_source_drift_requests_bounded_resynchronization() {
        let config = DriftScenarioConfig {
            ticks: 10_000,
            initial_occupancy_blocks: 4,
            source_rate_ppm: MAX_DRIFT_ADJUSTMENT_PPM + 500,
            controller: CONTROLLER,
        };

        let report = simulate_drift(config).unwrap();

        assert!(report.hard_resynchronizations > 0);
        assert_eq!(report.low_resynchronizations, 0);
        assert!(report.high_resynchronizations > 0);
    }

    #[test]
    fn uncorrectable_negative_source_drift_requests_low_side_resynchronization() {
        let config = DriftScenarioConfig {
            ticks: 10_000,
            initial_occupancy_blocks: 4,
            source_rate_ppm: -(MAX_DRIFT_ADJUSTMENT_PPM + 500),
            controller: CONTROLLER,
        };

        let report = simulate_drift(config).unwrap();

        assert!(report.hard_resynchronizations > 0);
        assert!(report.low_resynchronizations > 0);
        assert_eq!(report.high_resynchronizations, 0);
    }

    #[test]
    fn rejects_synthetic_input_outside_explicit_model_bounds() {
        let base = DriftScenarioConfig {
            ticks: 1,
            initial_occupancy_blocks: 4,
            source_rate_ppm: 0,
            controller: CONTROLLER,
        };

        assert_eq!(
            simulate_drift(DriftScenarioConfig { ticks: 0, ..base }),
            Err(DriftScenarioError::InvalidTickCount)
        );
        assert_eq!(
            simulate_drift(DriftScenarioConfig {
                initial_occupancy_blocks: u16::try_from(MAX_PCM_QUEUE_BLOCKS + 1).unwrap(),
                ..base
            }),
            Err(DriftScenarioError::InvalidInitialOccupancy)
        );
        assert_eq!(
            simulate_drift(DriftScenarioConfig {
                source_rate_ppm: MAX_SOURCE_RATE_PPM + 1,
                ..base
            }),
            Err(DriftScenarioError::InvalidSourceRate)
        );
        assert_eq!(
            simulate_drift(DriftScenarioConfig {
                source_rate_ppm: i32::MIN,
                ..base
            }),
            Err(DriftScenarioError::InvalidSourceRate)
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let controller = DriftScenarioError::Controller(DriftConfigError::InvalidGain);
        assert!(
            Error::source(&controller)
                .and_then(|source| source.downcast_ref::<DriftConfigError>())
                .is_some()
        );

        assert!(Error::source(&DriftScenarioError::InvalidTickCount).is_none());
    }
}
