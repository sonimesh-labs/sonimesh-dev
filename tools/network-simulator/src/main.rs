//! Deterministic virtual-network smoke runner for bounded audio buffering.

use sonimesh_network_simulator::{NetworkProfile, ScenarioConfig, simulate};

fn main() {
    println!("SoniMesh deterministic virtual-network profiles");
    for loss_percent in [0, 1, 3, 5] {
        let mut profile = NetworkProfile::stable(0x5A17_2026);
        profile.loss_percent = loss_percent;
        profile.jitter_ms = 8;
        profile.reorder_percent = 5;
        profile.duplicate_percent = 2;
        let report = simulate(ScenarioConfig {
            network: profile,
            ..ScenarioConfig::default()
        })
        .expect("built-in profiles are valid");

        println!(
            "loss={loss_percent:>2}% sent={} network_lost={} decoded={} plc={} duplicates={} late={} resets={}",
            report.network.submitted_packets,
            report.network.random_loss_packets + report.network.burst_loss_packets,
            report.playout_frames,
            report.plc_frames,
            report.duplicate_frames,
            report.late_frames,
            report.resynchronizations
        );
    }
}
