//! Deterministic integration coverage for the target-gated playout scheduler.
//!
//! The scheduler scenario (ADR 0016) gates playout on the adaptive target,
//! drains without gating once no frames are in flight, and forces a playout
//! when the gate deadline expires. These tests pin the deterministic
//! behavior: every observation is either one gated wait or one consumed
//! frame, so `gated_ticks + consumed == observations` always holds, and the
//! controller keeps the final target inside `[min, max]`.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterSchedulerScenarioConfig, JitterSchedulerScenarioError, NetworkProfile, ProfileError,
    simulate_jitter_scheduler,
};

/// Deterministic lossless link with fixed 20 ms base delay and no jitter.
fn lossless(seed: u64) -> NetworkProfile {
    let mut network = NetworkProfile::stable(seed);
    network.jitter_ms = 0;
    network.reorder_delay_ms = 0;
    network
}

#[test]
fn stable_lossless_profile_holds_the_target_and_never_gates() {
    let config = JitterSchedulerScenarioConfig {
        frames: 50,
        network: lossless(0x0A0C_2026),
        ..JitterSchedulerScenarioConfig::default()
    };

    let report = simulate_jitter_scheduler(config).unwrap();

    assert_eq!(report.playout.playout_frames, config.frames);
    assert_eq!(report.playout.plc_frames, 0);
    assert_eq!(report.gated_ticks, 0);
    assert_eq!(report.forced_playouts, 0);
    assert_eq!(report.excess_latency_ms, 0);
    assert_eq!(report.final_target_frames, report.base_frames);
    assert_eq!(report.raised_steps, 0);
    assert_eq!(report.lowered_steps, 0);
    assert_eq!(
        report.gated_ticks + report.playout.playout_frames + report.playout.plc_frames,
        report.observations
    );
}

#[test]
fn early_start_gates_until_the_target_is_reached() {
    let config = JitterSchedulerScenarioConfig {
        frames: 50,
        initial_playout_ms: 20,
        network: lossless(0x0A0C_2027),
        ..JitterSchedulerScenarioConfig::default()
    };

    let report = simulate_jitter_scheduler(config).unwrap();

    // Frames arrive one per 20 ms; the first tick at 20 ms sees one buffered
    // frame, the second at 40 ms sees two, and the third at 60 ms reaches the
    // Balanced 20 ms base target of three frames.
    assert_eq!(report.gated_ticks, 2);
    assert_eq!(report.forced_playouts, 0);
    assert_eq!(report.excess_latency_ms, 40);
    assert_eq!(report.playout.playout_frames, config.frames);
    assert_eq!(report.playout.plc_frames, 0);
    assert_eq!(report.final_target_frames, report.base_frames);
}

#[test]
fn burst_loss_becomes_plc_and_every_frame_is_consumed() {
    let mut network = lossless(0x0A0C_2028);
    network.burst_interval_packets = 20;
    network.burst_length_packets = 3;
    let config = JitterSchedulerScenarioConfig {
        frames: 100,
        network,
        ..JitterSchedulerScenarioConfig::default()
    };

    let report = simulate_jitter_scheduler(config).unwrap();

    // Bursts start at packet ordinal 20 and repeat every 20 packets.
    assert_eq!(report.network.burst_loss_packets, 12);
    assert_eq!(report.playout.plc_frames, 12);
    assert_eq!(
        report.playout.playout_frames + report.playout.plc_frames,
        config.frames
    );
    assert_eq!(report.playout.resynchronizations, 0);
    assert!(
        report.excess_latency_ms < 1_000,
        "latency must stay bounded"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "final target must stay inside the bounded range"
    );
    assert_eq!(
        report.gated_ticks + report.playout.playout_frames + report.playout.plc_frames,
        report.observations
    );
}

#[test]
fn high_jitter_raises_the_adaptive_target() {
    // A one-way delay spread of 100 ms around a 20 ms base keeps the smoothed
    // arrival-jitter estimate near 35 ms, above the LowLatency 10 ms base
    // target of three frames (30 ms), so the controller must raise the target.
    let mut network = lossless(0x0A0C_2029);
    network.jitter_ms = 100;
    let config = JitterSchedulerScenarioConfig {
        frames: 400,
        frame_duration_ms: 10,
        initial_playout_ms: 30,
        target: JitterTargetConfig::preset(JitterMode::LowLatency, 10, 64),
        network,
        ..JitterSchedulerScenarioConfig::default()
    };

    let report = simulate_jitter_scheduler(config).unwrap();

    assert!(
        report.raised_steps > 0,
        "sustained jitter must raise the target"
    );
    assert!(report.final_target_frames > report.base_frames);
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "final target must stay inside the bounded range"
    );
    assert_eq!(
        report.playout.playout_frames + report.playout.plc_frames,
        config.frames
    );
}

#[test]
fn scheduler_report_is_deterministic() {
    let mut network = NetworkProfile::stable(0x5A17_2026);
    network.loss_percent = 3;
    network.reorder_percent = 5;
    let config = JitterSchedulerScenarioConfig {
        frames: 120,
        network,
        ..JitterSchedulerScenarioConfig::default()
    };

    let first = simulate_jitter_scheduler(config).unwrap();
    let second = simulate_jitter_scheduler(config).unwrap();

    assert_eq!(first, second);
}

#[test]
fn rejects_invalid_configuration() {
    let invalid_frames = JitterSchedulerScenarioConfig {
        frames: 0,
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(invalid_frames),
        Err(JitterSchedulerScenarioError::InvalidFrameCount)
    );

    let unnegotiated_duration = JitterSchedulerScenarioConfig {
        frame_duration_ms: 15,
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(unnegotiated_duration),
        Err(JitterSchedulerScenarioError::InvalidFrameDuration)
    );

    let mismatched_duration = JitterSchedulerScenarioConfig {
        target: JitterTargetConfig::preset(JitterMode::Balanced, 10, 64),
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(mismatched_duration),
        Err(JitterSchedulerScenarioError::TargetFrameDurationMismatch)
    );

    let mismatched_capacity = JitterSchedulerScenarioConfig {
        target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 8),
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(mismatched_capacity),
        Err(JitterSchedulerScenarioError::TargetBufferCapacityMismatch)
    );

    let zero_capacity = JitterSchedulerScenarioConfig {
        target: JitterTargetConfig::preset(JitterMode::Balanced, 20, 0),
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(zero_capacity),
        Err(JitterSchedulerScenarioError::Target(
            sonimesh_audio::JitterTargetConfigError::ZeroCapacity
        ))
    );

    let late_start = JitterSchedulerScenarioConfig {
        initial_playout_ms: 1_001,
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(late_start),
        Err(JitterSchedulerScenarioError::InvalidInitialPlayout)
    );

    for max_gate_ticks in [0, 65] {
        let invalid_gate = JitterSchedulerScenarioConfig {
            max_gate_ticks,
            ..JitterSchedulerScenarioConfig::default()
        };
        assert_eq!(
            simulate_jitter_scheduler(invalid_gate),
            Err(JitterSchedulerScenarioError::InvalidMaxGateTicks)
        );
    }

    let zero_datagram = JitterSchedulerScenarioConfig {
        media_datagram_bytes: 0,
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(zero_datagram),
        Err(JitterSchedulerScenarioError::InvalidMediaDatagramBytes)
    );

    let mut network = NetworkProfile::stable(3);
    network.burst_length_packets = 1;
    let invalid_profile = JitterSchedulerScenarioConfig {
        network,
        ..JitterSchedulerScenarioConfig::default()
    };
    assert_eq!(
        simulate_jitter_scheduler(invalid_profile),
        Err(JitterSchedulerScenarioError::Profile(
            ProfileError::BurstIntervalMissing
        ))
    );
}

#[test]
fn preserved_error_variants_expose_their_causes() {
    use std::error::Error;

    let target =
        JitterSchedulerScenarioError::Target(sonimesh_audio::JitterTargetConfigError::ZeroCapacity);
    assert!(
        Error::source(&target)
            .and_then(|source| source.downcast_ref::<sonimesh_audio::JitterTargetConfigError>())
            .is_some()
    );

    let profile = JitterSchedulerScenarioError::Profile(ProfileError::DelayTooLarge);
    assert!(
        Error::source(&profile)
            .and_then(|source| source.downcast_ref::<ProfileError>())
            .is_some()
    );

    assert!(Error::source(&JitterSchedulerScenarioError::InvalidFrameCount).is_none());
    assert!(Error::source(&JitterSchedulerScenarioError::InvalidMaxGateTicks).is_none());
    assert!(Error::source(&JitterSchedulerScenarioError::SimulationDidNotFinish).is_none());
}
