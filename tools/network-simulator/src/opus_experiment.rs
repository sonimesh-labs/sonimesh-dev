use sonimesh_audio::OpusFrameConfig;

use crate::{NetworkProfile, ScenarioConfig, ScenarioError, SimulationReport, simulate};

/// One deterministic codec-neutral case from the proposed Opus experiment matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OpusExperimentCase {
    /// Stable experiment identifier matching the published matrix row.
    pub id: OpusExperimentId,
    /// Negotiated initial Opus PCM frame shape.
    pub frame: OpusFrameConfig,
    /// Proposed encoder bitrate for future quality/CPU comparison only.
    pub proposed_bitrate_bps: u32,
    /// Deterministic virtual network profile.
    pub network: NetworkProfile,
    /// Independent random-loss variant for C/D, when applicable.
    pub independent_loss_percent: Option<u8>,
}

/// Stable identifiers for the first codec-neutral Opus experiment cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpusExperimentId {
    /// Matrix cell A: 10 ms stable WLAN baseline.
    A10MsStable,
    /// Matrix cell B: 20 ms stable WLAN baseline.
    B20MsStable,
    /// Matrix cell C: 10 ms independent loss.
    C10MsLoss,
    /// Matrix cell D: 20 ms independent loss.
    D20MsLoss,
    /// Matrix cell E: 10 ms deterministic burst/reorder.
    E10MsBurstReorder,
    /// Matrix cell F: 20 ms deterministic burst/reorder.
    F20MsBurstReorder,
}

impl OpusExperimentId {
    /// Returns the short published experiment label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::A10MsStable => "A",
            Self::B20MsStable => "B",
            Self::C10MsLoss => "C",
            Self::D20MsLoss => "D",
            Self::E10MsBurstReorder => "E",
            Self::F20MsBurstReorder => "F",
        }
    }
}

/// Returns the fixed A-F codec-neutral experiment suite.
///
/// C/D each contain fixed 1% and 3% independent-loss runs. FEC and DTX are
/// absent by design. The bitrate field is report metadata, not an encoder
/// configuration or product default.
#[must_use]
pub fn opus_experiment_cases() -> [OpusExperimentCase; 8] {
    let stable = NetworkProfile::stable(0x0A0F_0001);
    let loss_one_percent = NetworkProfile {
        loss_percent: 1,
        seed: 0x0A0F_0002,
        ..NetworkProfile::stable(0x0A0F_0002)
    };
    let loss_three_percent = NetworkProfile {
        loss_percent: 3,
        seed: 0x0A0F_0004,
        ..NetworkProfile::stable(0x0A0F_0004)
    };
    let burst_reorder = NetworkProfile {
        reorder_percent: 25,
        reorder_delay_ms: 40,
        burst_interval_packets: 20,
        burst_length_packets: 3,
        seed: 0x0A0F_0003,
        ..NetworkProfile::stable(0x0A0F_0003)
    };

    [
        case(OpusExperimentId::A10MsStable, 10, 64_000, stable, None),
        case(OpusExperimentId::B20MsStable, 20, 64_000, stable, None),
        case(
            OpusExperimentId::C10MsLoss,
            10,
            96_000,
            loss_one_percent,
            Some(1),
        ),
        case(
            OpusExperimentId::D20MsLoss,
            20,
            96_000,
            loss_one_percent,
            Some(1),
        ),
        case(
            OpusExperimentId::C10MsLoss,
            10,
            96_000,
            loss_three_percent,
            Some(3),
        ),
        case(
            OpusExperimentId::D20MsLoss,
            20,
            96_000,
            loss_three_percent,
            Some(3),
        ),
        case(
            OpusExperimentId::E10MsBurstReorder,
            10,
            96_000,
            burst_reorder,
            None,
        ),
        case(
            OpusExperimentId::F20MsBurstReorder,
            20,
            96_000,
            burst_reorder,
            None,
        ),
    ]
}

/// Runs one fixed experiment case with fixed sender/playout cadence.
///
/// # Errors
///
/// Returns a scenario error when a future edit makes one fixed case violate the
/// deterministic simulator bounds.
pub fn simulate_opus_experiment(
    case: OpusExperimentCase,
) -> Result<SimulationReport, ScenarioError> {
    simulate(ScenarioConfig {
        frames: 500,
        frame_duration_ms: u64::from(case.frame.frame_duration_ms()),
        target_buffer_ms: u64::from(case.frame.frame_duration_ms()) * 3,
        network: case.network,
        ..ScenarioConfig::default()
    })
}

fn case(
    id: OpusExperimentId,
    frame_duration_ms: u16,
    proposed_bitrate_bps: u32,
    network: NetworkProfile,
    independent_loss_percent: Option<u8>,
) -> OpusExperimentCase {
    OpusExperimentCase {
        id,
        frame: frame(frame_duration_ms),
        proposed_bitrate_bps,
        network,
        independent_loss_percent,
    }
}

fn frame(frame_duration_ms: u16) -> OpusFrameConfig {
    OpusFrameConfig::new(48_000, 2, frame_duration_ms)
        .expect("built-in Opus experiment shape must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suite_matches_the_published_frame_bitrate_and_loss_matrix() {
        let cases = opus_experiment_cases();
        assert_eq!(cases.len(), 8);
        assert_eq!(cases[0].id, OpusExperimentId::A10MsStable);
        assert_eq!(cases[0].frame.frame_duration_ms(), 10);
        assert_eq!(cases[1].frame.frame_duration_ms(), 20);
        assert_eq!(cases[0].proposed_bitrate_bps, 64_000);
        assert_eq!(cases[1].proposed_bitrate_bps, 64_000);
        assert_eq!(cases[2].independent_loss_percent, Some(1));
        assert_eq!(cases[3].independent_loss_percent, Some(1));
        assert_eq!(cases[4].independent_loss_percent, Some(3));
        assert_eq!(cases[5].independent_loss_percent, Some(3));
        for case_ in &cases[2..] {
            assert_eq!(case_.proposed_bitrate_bps, 96_000);
        }
    }

    #[test]
    fn stable_cells_are_lossless_and_impaired_cells_exercise_plc() {
        for case_ in opus_experiment_cases() {
            let first = simulate_opus_experiment(case_).unwrap();
            let second = simulate_opus_experiment(case_).unwrap();
            assert_eq!(first, second);
            assert_eq!(first.playout_frames + first.plc_frames, 500);
            match case_.id {
                OpusExperimentId::A10MsStable | OpusExperimentId::B20MsStable => {
                    assert_eq!(first.plc_frames, 0);
                }
                OpusExperimentId::C10MsLoss | OpusExperimentId::D20MsLoss => {
                    assert!(matches!(case_.independent_loss_percent, Some(1 | 3)));
                    assert!(first.plc_frames > 0);
                }
                OpusExperimentId::E10MsBurstReorder | OpusExperimentId::F20MsBurstReorder => {
                    assert!(first.plc_frames > 0);
                }
            }
        }
    }
}
