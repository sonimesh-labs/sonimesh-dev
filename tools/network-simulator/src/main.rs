//! Deterministic virtual-network smoke runner for bounded audio buffering.

use sonimesh_audio::DriftConfig;
use sonimesh_network_simulator::{
    DriftScenarioConfig, NetworkProfile, ScenarioConfig, simulate, simulate_drift,
};

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

    let drift = simulate_drift(DriftScenarioConfig {
        ticks: 20_000,
        initial_occupancy_blocks: 5,
        source_rate_ppm: 100,
        controller: DriftConfig {
            target_blocks: 4,
            low_resync_blocks: 0,
            high_resync_blocks: 8,
            proportional_ppm_per_block: 25,
            integral_ppm_per_block_tick: 1,
            max_adjustment_ppm: 500,
            max_step_ppm: 25,
            max_integral_block_ticks: 256,
        },
    })
    .expect("built-in drift scenario is valid");
    println!(
        "drift source=100ppm final_occupancy={} final_adjustment={}ppm resyncs={}",
        drift.final_occupancy_blocks, drift.final_adjustment_ppm, drift.hard_resynchronizations
    );
}
