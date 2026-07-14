//! Deterministic virtual-network smoke runner for bounded audio buffering.

use std::collections::BTreeMap;

use sonimesh_audio::{EncodedFrame, JitterBuffer, PlayoutOutcome};

const FRAME_MS: u64 = 20;
const TARGET_BUFFER_MS: u64 = 60;
const PACKETS: u32 = 500;

#[derive(Clone, Copy)]
struct Profile {
    loss_percent: u32,
    base_delay_ms: u64,
    jitter_ms: u64,
    seed: u64,
}

#[derive(Default)]
struct Report {
    delivered: u32,
    lost_by_network: u32,
    playout_frames: u32,
    plc_frames: u32,
    late_frames: u32,
    resynchronizations: u32,
}

fn main() {
    println!("SoniMesh deterministic virtual-network smoke profiles");
    for loss_percent in [0, 1, 3, 5] {
        let report = simulate(Profile {
            loss_percent,
            base_delay_ms: 20,
            jitter_ms: 8,
            seed: 0x5A17_2026,
        });
        println!(
            "loss={loss_percent:>2}% delivered={} network_lost={} decoded={} plc={} late={} resets={}",
            report.delivered,
            report.lost_by_network,
            report.playout_frames,
            report.plc_frames,
            report.late_frames,
            report.resynchronizations
        );
    }
}

fn simulate(profile: Profile) -> Report {
    let mut random = Lcg::new(profile.seed);
    let mut arrivals: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    let mut report = Report::default();

    for sequence in 0..PACKETS {
        if random.below(100) < profile.loss_percent {
            report.lost_by_network += 1;
            continue;
        }
        let jitter_span = profile.jitter_ms.saturating_mul(2).saturating_add(1);
        let jitter = random.below(u32::try_from(jitter_span).unwrap_or(u32::MAX));
        let delay = profile
            .base_delay_ms
            .saturating_add(u64::from(jitter))
            .saturating_sub(profile.jitter_ms);
        let arrival = u64::from(sequence)
            .saturating_mul(FRAME_MS)
            .saturating_add(delay);
        arrivals.entry(arrival).or_default().push(sequence);
    }

    let mut buffer = JitterBuffer::<16, 8>::new();
    let end_ms = u64::from(PACKETS) * FRAME_MS + TARGET_BUFFER_MS + 200;
    for now_ms in 0..=end_ms {
        if let Some(sequences) = arrivals.remove(&now_ms) {
            for sequence in sequences {
                let frame = EncodedFrame::new(sequence, u64::from(sequence) * 960, &[0]).unwrap();
                match buffer.insert(frame) {
                    sonimesh_audio::InsertOutcome::Inserted => report.delivered += 1,
                    sonimesh_audio::InsertOutcome::Late => report.late_frames += 1,
                    sonimesh_audio::InsertOutcome::Resynchronized { .. } => {
                        report.delivered += 1;
                        report.resynchronizations += 1;
                    }
                    sonimesh_audio::InsertOutcome::Duplicate
                    | sonimesh_audio::InsertOutcome::Disabled => {}
                }
            }
        }

        if now_ms >= TARGET_BUFFER_MS && (now_ms - TARGET_BUFFER_MS) % FRAME_MS == 0 {
            match buffer.take_next() {
                PlayoutOutcome::Frame(_) => report.playout_frames += 1,
                PlayoutOutcome::Missing { .. } => report.plc_frames += 1,
                PlayoutOutcome::NotReady => {}
            }
        }
    }
    report
}

struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn below(&mut self, upper: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        if upper == 0 {
            return 0;
        }
        ((self.0 >> 32) as u32) % upper
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_is_deterministic_and_bounded() {
        let profile = Profile {
            loss_percent: 3,
            base_delay_ms: 20,
            jitter_ms: 8,
            seed: 42,
        };
        let first = simulate(profile);
        let second = simulate(profile);

        assert_eq!(first.delivered, second.delivered);
        assert_eq!(first.plc_frames, second.plc_frames);
        assert!(first.delivered <= PACKETS);
        assert_eq!(first.resynchronizations, 0);
    }
}
