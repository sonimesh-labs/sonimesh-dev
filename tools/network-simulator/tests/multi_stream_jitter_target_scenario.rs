//! Black-box coverage for the multi-stream adaptive-jitter-target simulation.

use sonimesh_network_simulator::{
    MultiStreamJitterTargetScenarioConfig, MultiStreamJitterTargetScenarioError, NetworkProfile,
    simulate_multi_stream_jitter_target,
};

#[test]
fn report_is_deterministic_and_every_stream_stays_bounded() {
    let mut network = NetworkProfile::stable(0xC0FF_EE00);
    network.loss_percent = 4;
    network.duplicate_percent = 3;
    network.reorder_percent = 7;
    let config = MultiStreamJitterTargetScenarioConfig {
        frames: 300,
        network,
        ..MultiStreamJitterTargetScenarioConfig::default()
    };

    let first = simulate_multi_stream_jitter_target::<3>(config).unwrap();
    let second = simulate_multi_stream_jitter_target::<3>(config).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.streams.len(), 3);
    for stream in &first.streams {
        assert_eq!(stream.observations, config.frames);
        assert_eq!(
            stream.playout.playout_frames + stream.playout.plc_frames,
            config.frames
        );
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
            "target left the bounded water-level range"
        );
    }
    assert!(first.network.random_loss_packets > 0);
    let total_plc: u32 = first
        .streams
        .iter()
        .map(|stream| stream.playout.plc_frames)
        .sum();
    assert!(total_plc > 0);
}

#[test]
fn shared_burst_impairs_every_stream_within_target_bounds() {
    let mut network = NetworkProfile::stable(0xB0C0_2026);
    network.burst_interval_packets = 25;
    network.burst_length_packets = 2;
    let config = MultiStreamJitterTargetScenarioConfig {
        frames: 50,
        network,
        ..MultiStreamJitterTargetScenarioConfig::default()
    };

    let report = simulate_multi_stream_jitter_target::<2>(config).unwrap();

    assert!(report.network.burst_loss_packets > 0);
    for stream in &report.streams {
        assert!(stream.playout.plc_frames > 0);
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
            "target left the bounded water-level range"
        );
    }
}

#[test]
fn per_stream_modes_produce_independent_water_levels() {
    let mut network = NetworkProfile::stable(0xDEAD_BEEF);
    network.loss_percent = 2;
    network.reorder_percent = 5;
    let config = MultiStreamJitterTargetScenarioConfig {
        frames: 200,
        network,
        targets: [
            sonimesh_audio::JitterTargetConfig::preset(
                sonimesh_audio::JitterMode::LowLatency,
                20,
                64,
            ),
            sonimesh_audio::JitterTargetConfig::preset(sonimesh_audio::JitterMode::Stable, 20, 64),
        ],
        ..MultiStreamJitterTargetScenarioConfig::default()
    };

    let report = simulate_multi_stream_jitter_target::<2>(config).unwrap();

    assert!(report.streams[0].max_frames < report.streams[1].max_frames);
    for stream in &report.streams {
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
            "target left the bounded water-level range"
        );
    }
}

#[test]
fn clean_shared_link_holds_every_mode_base_across_four_streams() {
    let mut network = NetworkProfile::stable(0x0A0C_2026);
    network.base_delay_ms = 0;
    network.jitter_ms = 0;
    let config = MultiStreamJitterTargetScenarioConfig {
        frames: 7,
        frame_duration_ms: 10,
        target_buffer_ms: 10,
        network,
        targets: std::array::from_fn(|_| {
            sonimesh_audio::JitterTargetConfig::preset(sonimesh_audio::JitterMode::Balanced, 10, 64)
        }),
        ..MultiStreamJitterTargetScenarioConfig::default()
    };

    let report = simulate_multi_stream_jitter_target::<4>(config).unwrap();

    for stream in &report.streams {
        assert_eq!(stream.playout.playout_frames, config.frames);
        assert_eq!(stream.playout.plc_frames, 0);
        assert_eq!(stream.final_target_frames, stream.base_frames);
        assert_eq!(stream.raised_steps, 0);
        assert_eq!(stream.lowered_steps, 0);
    }
}

#[test]
fn mismatched_controller_configuration_is_rejected() {
    let config = MultiStreamJitterTargetScenarioConfig::default();
    assert_eq!(
        simulate_multi_stream_jitter_target::<2>(MultiStreamJitterTargetScenarioConfig {
            targets: [
                sonimesh_audio::JitterTargetConfig::preset(
                    sonimesh_audio::JitterMode::Balanced,
                    10,
                    64,
                ),
                sonimesh_audio::JitterTargetConfig::preset(
                    sonimesh_audio::JitterMode::Balanced,
                    20,
                    64,
                ),
            ],
            ..config
        }),
        Err(MultiStreamJitterTargetScenarioError::TargetFrameDurationMismatch)
    );
}
