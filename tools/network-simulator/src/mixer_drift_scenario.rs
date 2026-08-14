//! Deterministic per-stream drift isolation over the Phase 2 source set.
//!
//! Two to four independent source streams each own an integerized
//! PCM-occupancy plant and a private [`DriftController`] with the same
//! validated bounds, mirroring the per-stream controllers a Phase 2 mixer
//! session owns (ADR 0007). The report carries one occupancy/decision summary
//! per stream, so the scenario
//! proves the ADR 0007 per-stream isolation property deterministically: a
//! stream's occupancy, ratio requests, and hard-resynchronization bookkeeping
//! are functions of only that stream's source rate and initial occupancy, a
//! clean stream never requests a resynchronization even while an uncorrectable
//! sibling stream does, and every stream's report equals the single-stream
//! [`simulate_drift`] result for the same per-stream configuration. It is not
//! an ASRC or device-clock emulator and makes no physical latency claim.

use core::fmt;

use sonimesh_audio::{DriftConfig, DriftConfigError, DriftController, MAX_PCM_QUEUE_BLOCKS};

use crate::drift_scenario::{DriftSimulationReport, simulate_plant};

/// Fewest independent source streams accepted by the mixer-drift scenario.
pub const MIN_MIXER_DRIFT_STREAMS: usize = 2;
/// Most independent source streams accepted by the mixer-drift scenario.
pub const MAX_MIXER_DRIFT_STREAMS: usize = 4;
/// Hard bound on plant ticks per stream, shared with the single-stream scenario.
const MAX_MIXER_DRIFT_TICKS: u32 = 100_000;
/// The model's synthetic sender-rate bound, shared with the single-stream scenario.
const MAX_SOURCE_RATE_PPM: i32 = 10_000;

/// One independent source stream's synthetic clock relationship.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerDriftStreamConfig {
    /// Initial occupancy snapshot supplied to this stream's controller.
    pub initial_occupancy_blocks: u16,
    /// Sender clock offset from nominal in ppm for this stream.
    pub source_rate_ppm: i32,
}

/// One deterministic multi-stream mixer-drift configuration.
///
/// `STREAMS` independent source streams run in parallel; every stream uses a
/// private [`DriftController`] instance with the same `controller` bounds, so
/// the report can prove per-stream isolation and single-stream equivalence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerDriftScenarioConfig<const STREAMS: usize> {
    /// Number of fixed-duration PCM block ticks to simulate per stream.
    pub ticks: u32,
    /// Bounded per-stream controller configuration shared by every stream.
    pub controller: DriftConfig,
    /// Per-stream initial occupancy and synthetic source rate.
    pub streams: [MixerDriftStreamConfig; STREAMS],
}

/// Deterministic closed-loop result for one stream of a mixer-drift run.
///
/// Each entry reuses the single-stream [`DriftSimulationReport`] shape so the
/// scenario can assert per-stream equivalence to [`simulate_drift`] directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MixerDriftSimulationReport<const STREAMS: usize> {
    /// Per-stream occupancy and decision summaries in stream order.
    pub streams: [DriftSimulationReport; STREAMS],
}

/// Invalid deterministic multi-stream mixer-drift configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixerDriftScenarioError {
    /// Stream count is outside the explicit two-to-four Phase 2 bound.
    InvalidStreamCount,
    /// Tick count is zero or exceeds the simulation hard bound.
    InvalidTickCount,
    /// A stream's initial occupancy cannot fit the bounded PCM queue model.
    InvalidInitialOccupancy,
    /// A stream's synthetic sender offset exceeds the model hard bound.
    InvalidSourceRate,
    /// The shared per-stream controller configuration is invalid.
    Controller(DriftConfigError),
}

impl fmt::Display for MixerDriftScenarioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidStreamCount => formatter.write_str(
                "invalid mixer-drift scenario: stream count is outside the two-to-four bound",
            ),
            Self::InvalidTickCount => formatter.write_str(
                "invalid mixer-drift scenario: tick count is outside the explicit bound",
            ),
            Self::InvalidInitialOccupancy => formatter.write_str(
                "invalid mixer-drift scenario: initial occupancy exceeds the bounded PCM queue",
            ),
            Self::InvalidSourceRate => formatter.write_str(
                "invalid mixer-drift scenario: source rate is outside the explicit model bound",
            ),
            Self::Controller(error) => {
                write!(
                    formatter,
                    "invalid mixer-drift scenario controller: {error}"
                )
            }
        }
    }
}

impl std::error::Error for MixerDriftScenarioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Controller(error) => Some(error),
            Self::InvalidStreamCount
            | Self::InvalidTickCount
            | Self::InvalidInitialOccupancy
            | Self::InvalidSourceRate => None,
        }
    }
}

/// Runs one deterministic multi-stream mixer-drift simulation.
///
/// Every stream runs the shared integerized plant from the single-stream
/// scenario through its own [`DriftController`], so the report aggregates
/// independent per-stream decisions without any cross-stream state. Each
/// stream's plant, occupancy, ratio requests, and resynchronization
/// bookkeeping therefore stay functions of only that stream's configuration.
///
/// # Errors
///
/// Returns [`MixerDriftScenarioError`] when the stream count, synthetic tick,
/// per-stream occupancy or source rate, or shared controller configuration is
/// outside its explicit bound.
pub fn simulate_mixer_drift<const STREAMS: usize>(
    config: MixerDriftScenarioConfig<STREAMS>,
) -> Result<MixerDriftSimulationReport<STREAMS>, MixerDriftScenarioError> {
    validate(config)?;
    let mut streams = [DriftSimulationReport {
        minimum_occupancy_blocks: 0,
        maximum_occupancy_blocks: 0,
        final_occupancy_blocks: 0,
        final_adjustment_ppm: 0,
        hard_resynchronizations: 0,
        low_resynchronizations: 0,
        high_resynchronizations: 0,
    }; STREAMS];
    for (index, stream) in config.streams.iter().enumerate() {
        let mut controller =
            DriftController::new(config.controller).map_err(MixerDriftScenarioError::Controller)?;
        streams[index] = simulate_plant(
            config.ticks,
            stream.initial_occupancy_blocks,
            stream.source_rate_ppm,
            config.controller.target_blocks,
            &mut controller,
        );
    }
    Ok(MixerDriftSimulationReport { streams })
}

fn validate<const STREAMS: usize>(
    config: MixerDriftScenarioConfig<STREAMS>,
) -> Result<(), MixerDriftScenarioError> {
    if !(MIN_MIXER_DRIFT_STREAMS..=MAX_MIXER_DRIFT_STREAMS).contains(&STREAMS) {
        return Err(MixerDriftScenarioError::InvalidStreamCount);
    }
    if config.ticks == 0 || config.ticks > MAX_MIXER_DRIFT_TICKS {
        return Err(MixerDriftScenarioError::InvalidTickCount);
    }
    for stream in &config.streams {
        if usize::from(stream.initial_occupancy_blocks) > MAX_PCM_QUEUE_BLOCKS {
            return Err(MixerDriftScenarioError::InvalidInitialOccupancy);
        }
        if i64::from(stream.source_rate_ppm).abs() > i64::from(MAX_SOURCE_RATE_PPM) {
            return Err(MixerDriftScenarioError::InvalidSourceRate);
        }
    }
    config
        .controller
        .validate()
        .map_err(MixerDriftScenarioError::Controller)
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

    const CLEAN: MixerDriftStreamConfig = MixerDriftStreamConfig {
        initial_occupancy_blocks: 4,
        source_rate_ppm: 0,
    };

    const CORRECTABLE_HIGH: MixerDriftStreamConfig = MixerDriftStreamConfig {
        initial_occupancy_blocks: 5,
        source_rate_ppm: 100,
    };

    const CORRECTABLE_LOW: MixerDriftStreamConfig = MixerDriftStreamConfig {
        initial_occupancy_blocks: 3,
        source_rate_ppm: -80,
    };

    const UNCORRECTABLE_HIGH: MixerDriftStreamConfig = MixerDriftStreamConfig {
        initial_occupancy_blocks: 4,
        source_rate_ppm: MAX_DRIFT_ADJUSTMENT_PPM + 500,
    };

    fn config<const STREAMS: usize>(
        streams: [MixerDriftStreamConfig; STREAMS],
    ) -> MixerDriftScenarioConfig<STREAMS> {
        MixerDriftScenarioConfig {
            ticks: 20_000,
            controller: CONTROLLER,
            streams,
        }
    }

    #[test]
    fn multi_stream_loop_is_deterministic_and_bounded() {
        let first = simulate_mixer_drift(config([
            CORRECTABLE_HIGH,
            CORRECTABLE_LOW,
            UNCORRECTABLE_HIGH,
        ]))
        .unwrap();
        let second = simulate_mixer_drift(config([
            CORRECTABLE_HIGH,
            CORRECTABLE_LOW,
            UNCORRECTABLE_HIGH,
        ]))
        .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.streams[0].hard_resynchronizations, 0);
        assert_eq!(first.streams[1].hard_resynchronizations, 0);
        assert!(first.streams[2].hard_resynchronizations > 0);
        for report in &first.streams {
            assert!(report.final_adjustment_ppm.abs() <= MAX_DRIFT_ADJUSTMENT_PPM);
        }
    }

    #[test]
    fn each_stream_matches_its_single_stream_loop() {
        let streams = [CORRECTABLE_HIGH, CORRECTABLE_LOW, UNCORRECTABLE_HIGH];
        let report = simulate_mixer_drift(config(streams)).unwrap();
        for (index, stream) in streams.iter().enumerate() {
            let single =
                crate::drift_scenario::simulate_drift(crate::drift_scenario::DriftScenarioConfig {
                    ticks: 20_000,
                    initial_occupancy_blocks: stream.initial_occupancy_blocks,
                    source_rate_ppm: stream.source_rate_ppm,
                    controller: CONTROLLER,
                })
                .unwrap();
            assert_eq!(
                report.streams[index], single,
                "stream {index} must match the single-stream loop for its own configuration"
            );
        }
    }

    #[test]
    fn a_clean_stream_is_isolated_from_uncorrectable_siblings() {
        let report =
            simulate_mixer_drift(config([CLEAN, UNCORRECTABLE_HIGH, UNCORRECTABLE_HIGH])).unwrap();

        assert!(report.streams[1].hard_resynchronizations > 0);
        assert!(report.streams[2].hard_resynchronizations > 0);
        assert_eq!(
            report.streams[0].hard_resynchronizations, 0,
            "a clean stream must not resynchronize while sibling streams do"
        );
        assert_eq!(report.streams[0].final_adjustment_ppm, 0);
        assert_eq!(
            report.streams[0].final_occupancy_blocks, CONTROLLER.target_blocks,
            "a clean stream must stay at the target occupancy"
        );
    }

    #[test]
    fn rejects_synthetic_input_outside_explicit_model_bounds() {
        let base = config([CLEAN, CORRECTABLE_HIGH]);

        assert_eq!(
            simulate_mixer_drift(MixerDriftScenarioConfig { ticks: 0, ..base }),
            Err(MixerDriftScenarioError::InvalidTickCount)
        );
        assert_eq!(
            simulate_mixer_drift(MixerDriftScenarioConfig {
                ticks: MAX_MIXER_DRIFT_TICKS + 1,
                ..base
            }),
            Err(MixerDriftScenarioError::InvalidTickCount)
        );
        assert_eq!(
            simulate_mixer_drift(MixerDriftScenarioConfig {
                streams: [
                    MixerDriftStreamConfig {
                        initial_occupancy_blocks: u16::try_from(MAX_PCM_QUEUE_BLOCKS + 1).unwrap(),
                        ..CLEAN
                    },
                    CORRECTABLE_HIGH,
                ],
                ..base
            }),
            Err(MixerDriftScenarioError::InvalidInitialOccupancy)
        );
        assert_eq!(
            simulate_mixer_drift(MixerDriftScenarioConfig {
                streams: [
                    MixerDriftStreamConfig {
                        source_rate_ppm: MAX_SOURCE_RATE_PPM + 1,
                        ..CLEAN
                    },
                    CORRECTABLE_HIGH,
                ],
                ..base
            }),
            Err(MixerDriftScenarioError::InvalidSourceRate)
        );
        assert_eq!(
            simulate_mixer_drift(MixerDriftScenarioConfig {
                controller: DriftConfig {
                    low_resync_blocks: CONTROLLER.target_blocks,
                    ..CONTROLLER
                },
                ..base
            }),
            Err(MixerDriftScenarioError::Controller(
                DriftConfigError::InvalidOccupancyThresholds
            ))
        );
    }

    #[test]
    fn preserved_error_variants_expose_their_causes() {
        let controller = MixerDriftScenarioError::Controller(DriftConfigError::InvalidGain);
        assert!(
            Error::source(&controller)
                .and_then(|source| source.downcast_ref::<DriftConfigError>())
                .is_some()
        );

        for error in [
            MixerDriftScenarioError::InvalidStreamCount,
            MixerDriftScenarioError::InvalidTickCount,
            MixerDriftScenarioError::InvalidInitialOccupancy,
            MixerDriftScenarioError::InvalidSourceRate,
        ] {
            assert!(Error::source(&error).is_none());
        }
    }
}
