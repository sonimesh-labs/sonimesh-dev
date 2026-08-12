//! Black-box coverage for the multi-stream shared-link simulation.

use sonimesh_network_simulator::{
    MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS, MultiStreamScenarioConfig,
    MultiStreamScenarioError, NetworkProfile, simulate_multi_stream,
};

#[test]
fn multi_stream_report_is_deterministic_and_accounts_every_playout() {
    let mut network = NetworkProfile::stable(0xC0FF_EE00);
    network.loss_percent = 4;
    network.duplicate_percent = 3;
    network.reorder_percent = 7;
    let config = MultiStreamScenarioConfig {
        frames: 300,
        network,
        ..MultiStreamScenarioConfig::default()
    };

    let first = simulate_multi_stream::<3>(config).unwrap();
    let second = simulate_multi_stream::<3>(config).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.streams.len(), 3);
    for stream in &first.streams {
        assert_eq!(stream.playout_frames + stream.plc_frames, config.frames);
    }
    let total_plc: u32 = first.streams.iter().map(|stream| stream.plc_frames).sum();
    assert!(total_plc > 0);
}

#[test]
fn shared_burst_impairs_every_stream_on_the_shared_link() {
    let mut network = NetworkProfile::stable(0xB0C0_2026);
    network.burst_interval_packets = 25;
    network.burst_length_packets = 2;
    let config = MultiStreamScenarioConfig {
        frames: 50,
        network,
        ..MultiStreamScenarioConfig::default()
    };

    let report = simulate_multi_stream::<2>(config).unwrap();

    assert!(report.network.burst_loss_packets > 0);
    assert!(report.streams[0].plc_frames > 0);
    assert!(report.streams[1].plc_frames > 0);
    let total_plc: u32 = report.streams.iter().map(|stream| stream.plc_frames).sum();
    assert_eq!(u64::from(total_plc), report.network.burst_loss_packets);
}

#[test]
fn stream_count_bounds_are_public_and_enforced() {
    assert_eq!(MIN_MULTI_STREAM_STREAMS, 2);
    assert_eq!(MAX_MULTI_STREAM_STREAMS, 4);

    let config = MultiStreamScenarioConfig::default();
    assert_eq!(
        simulate_multi_stream::<1>(config),
        Err(MultiStreamScenarioError::InvalidStreamCount)
    );
    let report = simulate_multi_stream::<4>(config).unwrap();
    assert_eq!(report.streams.len(), 4);
}
