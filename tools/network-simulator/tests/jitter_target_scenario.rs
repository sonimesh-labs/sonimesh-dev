//! Black-box coverage for the adaptive-jitter-target simulation.

use sonimesh_network_simulator::{
    JitterTargetScenarioConfig, JitterTargetScenarioError, NetworkProfile, simulate_jitter_target,
};

#[test]
fn report_is_deterministic_bounded_and_accounts_every_playout() {
    let mut network = NetworkProfile::stable(0xC0FF_EE00);
    network.loss_percent = 4;
    network.duplicate_percent = 3;
    network.reorder_percent = 7;
    let config = JitterTargetScenarioConfig {
        frames: 300,
        network,
        ..JitterTargetScenarioConfig::default()
    };

    let first = simulate_jitter_target(config).unwrap();
    let second = simulate_jitter_target(config).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.observations, config.frames);
    assert_eq!(
        first.playout.playout_frames + first.playout.plc_frames,
        config.frames
    );
    assert!(
        (first.min_frames..=first.max_frames).contains(&first.final_target_frames),
        "target left the bounded water-level range"
    );
    assert!(first.network.random_loss_packets > 0);
}

#[test]
fn sustained_high_jitter_drives_the_target_toward_max() {
    let mut network = NetworkProfile::stable(0xB0C0_2026);
    network.reorder_percent = 100;
    network.reorder_delay_ms = 120;
    let config = JitterTargetScenarioConfig {
        frames: 100,
        target_buffer_ms: 200,
        network,
        ..JitterTargetScenarioConfig::default()
    };

    let report = simulate_jitter_target(config).unwrap();

    assert_eq!(report.final_target_frames, report.max_frames);
    assert!(report.raised_steps > 0);
}

#[test]
fn sustained_low_jitter_with_surplus_drives_the_target_toward_min() {
    let mut network = NetworkProfile::stable(0xDEAD_BEEF);
    network.base_delay_ms = 0;
    network.jitter_ms = 0;
    let config = JitterTargetScenarioConfig {
        frames: 100,
        target_buffer_ms: 100,
        network,
        ..JitterTargetScenarioConfig::default()
    };

    let report = simulate_jitter_target(config).unwrap();

    assert_eq!(report.final_target_frames, report.min_frames);
    assert!(report.lowered_steps > 0);
    assert_eq!(report.raised_steps, 0);
}

#[test]
fn stable_profile_holds_the_mode_base() {
    let config = JitterTargetScenarioConfig {
        frames: 200,
        ..JitterTargetScenarioConfig::default()
    };

    let report = simulate_jitter_target(config).unwrap();

    assert_eq!(report.final_target_frames, report.base_frames);
    assert_eq!(report.raised_steps, 0);
    assert_eq!(report.lowered_steps, 0);
}

#[test]
fn mismatched_controller_configuration_is_rejected() {
    let config = JitterTargetScenarioConfig::default();
    assert_eq!(
        simulate_jitter_target(JitterTargetScenarioConfig {
            target: sonimesh_audio::JitterTargetConfig::preset(
                sonimesh_audio::JitterMode::Balanced,
                10,
                64,
            ),
            ..config
        }),
        Err(JitterTargetScenarioError::TargetFrameDurationMismatch)
    );
}
