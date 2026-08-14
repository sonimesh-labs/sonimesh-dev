use core::convert::Infallible;

use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, OpusFrameConfig, RealtimePcmConsumer,
    RealtimePcmPopOutcome, RealtimePcmPushOutcome, StreamDecodeWorker, realtime_pcm_queue,
};

use crate::{
    NetworkProfile, NetworkStats, ScenarioConfig, ScenarioError, SimulationReport, VirtualClock,
    VirtualNetwork, simulate,
};

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

/// Per-case result from the codec-neutral decode-worker experiment path.
///
/// This records only worker scheduling and queue mechanics for a mock decoder.
/// It is not an Opus quality, CPU, power, or latency measurement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OpusWorkerExperimentReport {
    /// Virtual transport counters from the shared deterministic profile.
    pub network: NetworkStats,
    /// Frames passed to the mock decoder through `StreamDecodeWorker`.
    pub decoded_frames: u32,
    /// Missing deadlines handled by `FrameDecoder::decode_plc` in the worker.
    pub plc_frames: u32,
    /// Stream-local jitter resynchronizations followed by worker reset.
    pub resynchronizations: u32,
    /// Largest number of PCM blocks observed before the paired consumer read.
    pub queued_pcm_high_water_blocks: u16,
}

/// Runs the fixed A-F profile through a bounded mock decoder and PCM queue.
///
/// The negotiated `OpusFrameConfig` validates the compile-time 10 ms or 20 ms
/// PCM capacity at worker construction. The paired consumer only copies queued
/// PCM blocks; it never invokes the decoder or PLC.
///
/// # Errors
///
/// Returns a scenario error if a built-in profile violates fixed worker or
/// virtual-clock invariants.
pub fn simulate_opus_worker_experiment(
    case: OpusExperimentCase,
) -> Result<OpusWorkerExperimentReport, ScenarioError> {
    match case.frame.frame_duration_ms() {
        10 => run_worker_case::<960>(case),
        20 => run_worker_case::<1_920>(case),
        _ => Err(ScenarioError::InvalidFrameDuration),
    }
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
    simulate(scenario_config(case))
}

fn scenario_config(case: OpusExperimentCase) -> ScenarioConfig {
    ScenarioConfig {
        frames: 500,
        frame_duration_ms: u64::from(case.frame.frame_duration_ms()),
        target_buffer_ms: u64::from(case.frame.frame_duration_ms()) * 3,
        network: case.network,
        ..ScenarioConfig::default()
    }
}

fn run_worker_case<const PCM_SAMPLES_PER_BLOCK: usize>(
    case: OpusExperimentCase,
) -> Result<OpusWorkerExperimentReport, ScenarioError> {
    let config = scenario_config(case);
    let mut network = VirtualNetwork::new(config.network).map_err(ScenarioError::Profile)?;
    let mut clock = VirtualClock::new();
    let (producer, mut consumer) = realtime_pcm_queue::<PCM_SAMPLES_PER_BLOCK>(2)
        .map_err(|_| ScenarioError::InternalCapacityInvariant)?;
    let mut worker = StreamDecodeWorker::<
        MockOpusDecoder<PCM_SAMPLES_PER_BLOCK>,
        64,
        8,
        PCM_SAMPLES_PER_BLOCK,
    >::new_for_opus(MockOpusDecoder, producer, 0, case.frame)
    .map_err(|_| ScenarioError::InternalCapacityInvariant)?;
    let mut report = OpusWorkerExperimentReport::default();
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
            admit_worker_delivery(&mut worker, &mut report, case, delivery.payload)?;
        }

        if playout_decisions < config.frames && now_ms == next_playout_ms {
            decode_and_consume(&mut worker, &mut consumer, &mut report)?;
            playout_decisions += 1;
            next_playout_ms = next_playout_ms.saturating_add(config.frame_duration_ms);
        }
        clock.advance_by(1).map_err(ScenarioError::Clock)?;
    }

    if playout_decisions != config.frames || !consumer.is_empty() {
        return Err(ScenarioError::InternalCapacityInvariant);
    }
    report.network = network.stats();
    Ok(report)
}

fn admit_worker_delivery<const PCM_SAMPLES_PER_BLOCK: usize>(
    worker: &mut StreamDecodeWorker<
        MockOpusDecoder<PCM_SAMPLES_PER_BLOCK>,
        64,
        8,
        PCM_SAMPLES_PER_BLOCK,
    >,
    report: &mut OpusWorkerExperimentReport,
    case: OpusExperimentCase,
    sequence: u32,
) -> Result<(), ScenarioError> {
    let payload = [sequence.to_le_bytes()[0]];
    match worker
        .admit(
            sequence,
            u64::from(sequence).saturating_mul(case.frame.samples_per_channel() as u64),
            &payload,
        )
        .map_err(|_| ScenarioError::InternalCapacityInvariant)?
    {
        InsertOutcome::Resynchronized { .. } => {
            worker.reset_after_jitter_resynchronization();
            report.resynchronizations += 1;
        }
        InsertOutcome::SequenceExhausted | InsertOutcome::Disabled => {
            return Err(ScenarioError::InternalCapacityInvariant);
        }
        InsertOutcome::Inserted
        | InsertOutcome::Reordered
        | InsertOutcome::Duplicate
        | InsertOutcome::Late => {}
    }
    Ok(())
}

fn decode_and_consume<const PCM_SAMPLES_PER_BLOCK: usize>(
    worker: &mut StreamDecodeWorker<
        MockOpusDecoder<PCM_SAMPLES_PER_BLOCK>,
        64,
        8,
        PCM_SAMPLES_PER_BLOCK,
    >,
    consumer: &mut RealtimePcmConsumer<PCM_SAMPLES_PER_BLOCK>,
    report: &mut OpusWorkerExperimentReport,
) -> Result<(), ScenarioError> {
    match worker
        .decode_next()
        .map_err(|_| ScenarioError::InternalCapacityInvariant)?
    {
        DecodeStep::Decoded { samples, queue, .. } => {
            if samples != PCM_SAMPLES_PER_BLOCK || queue != RealtimePcmPushOutcome::Inserted {
                return Err(ScenarioError::InternalCapacityInvariant);
            }
            report.decoded_frames += 1;
        }
        DecodeStep::Concealed { samples, queue, .. } => {
            if samples != PCM_SAMPLES_PER_BLOCK || queue != RealtimePcmPushOutcome::Inserted {
                return Err(ScenarioError::InternalCapacityInvariant);
            }
            report.plc_frames += 1;
        }
        // The worker is pre-armed at the negotiated sequence, so a decode
        // before the first frame arrives conceals via PLC; reaching NotReady
        // here means the wiring lost the start sequence.
        DecodeStep::NotReady => return Err(ScenarioError::InternalCapacityInvariant),
    }
    report.queued_pcm_high_water_blocks = report.queued_pcm_high_water_blocks.max(
        u16::try_from(worker.queued_blocks())
            .map_err(|_| ScenarioError::InternalCapacityInvariant)?,
    );
    let mut output = [0.0_f32; PCM_SAMPLES_PER_BLOCK];
    match consumer.pop_into(&mut output) {
        RealtimePcmPopOutcome::Filled {
            samples,
            discarded_stale_blocks: 0,
            ..
        } if samples == PCM_SAMPLES_PER_BLOCK => Ok(()),
        RealtimePcmPopOutcome::Filled { .. } | RealtimePcmPopOutcome::Underflow { .. } => {
            Err(ScenarioError::InternalCapacityInvariant)
        }
    }
}

struct MockOpusDecoder<const PCM_SAMPLES_PER_BLOCK: usize>;

impl<const PCM_SAMPLES_PER_BLOCK: usize> FrameDecoder for MockOpusDecoder<PCM_SAMPLES_PER_BLOCK> {
    type Error = Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        debug_assert_eq!(encoded.len(), 1);
        output.fill(f32::from(encoded[0]));
        Ok(PCM_SAMPLES_PER_BLOCK)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output.fill(0.0);
        Ok(PCM_SAMPLES_PER_BLOCK)
    }
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
    fn every_published_case_exercises_the_bounded_mock_decode_worker() {
        for case_ in opus_experiment_cases() {
            let report = simulate_opus_worker_experiment(case_).unwrap();
            assert_eq!(report.decoded_frames + report.plc_frames, 500);
            assert_eq!(report.queued_pcm_high_water_blocks, 1);
        }
    }

    #[test]
    fn stable_cases_need_no_mock_worker_plc_or_resynchronization() {
        for case_ in &opus_experiment_cases()[..2] {
            let report = simulate_opus_worker_experiment(*case_).unwrap();
            assert_eq!(report.plc_frames, 0);
            assert_eq!(report.resynchronizations, 0);
        }
    }

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
