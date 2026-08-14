#![no_main]

//! Fuzz the multi-stream shared-link scenario.
//!
//! Arbitrary bounded scenario configurations drive `simulate_multi_stream` end
//! to end: two source streams traverse one shared impaired virtual link and
//! each stream plays out through its own jitter buffer on its own deadline. An
//! accepted configuration must submit exactly one packet per stream per frame
//! on the shared link, account for every playout decision (play out or conceal
//! exactly the configured frame count), and stay deterministic; a rejected
//! configuration must return a clean scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_network_simulator::{
    MultiStreamScenarioConfig, MultiStreamSimulationReport, NetworkProfile, simulate_multi_stream,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Two source streams share one virtual link.
const STREAMS: usize = 2;
/// Sender frames per stream stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 200;

fuzz_target!(|data: &[u8]| {
    let frame_duration_ms = if data.first().is_some_and(|byte| byte % 2 == 0) {
        10_u64
    } else {
        20_u64
    };
    let frames = u32::from(u16::from_le_bytes([
        data.get(1).copied().unwrap_or(0),
        data.get(2).copied().unwrap_or(0),
    ])) % MAX_FUZZ_FRAMES
        + 1;
    let network = profile_from_bytes(data);
    // The initial playout target covers the worst-case first-arrival delay so
    // an intact first packet is always ready by the first playout deadline.
    let worst_case_first_arrival_ms =
        network.base_delay_ms + network.jitter_ms + network.reorder_delay_ms;
    let target_buffer_ms = worst_case_first_arrival_ms
        + u64::from(
            u32::from(u16::from_le_bytes([
                data.get(12).copied().unwrap_or(0),
                data.get(13).copied().unwrap_or(0),
            ])) % (1_001 - u32::try_from(worst_case_first_arrival_ms).unwrap()),
        );
    let media_datagram_bytes = u16::try_from(
        u32::from(u16::from_le_bytes([
            data.get(10).copied().unwrap_or(0),
            data.get(11).copied().unwrap_or(0),
        ])) % u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap()
            + 1,
    )
    .expect("the media datagram bound fits u16");
    let config = MultiStreamScenarioConfig {
        frames,
        frame_duration_ms,
        target_buffer_ms,
        media_datagram_bytes,
        network,
    };

    // Playout is pre-armed at the negotiated start sequence, so a lost or
    // late first packet conceals via PLC rather than reporting NotReady;
    // accepted configurations must satisfy the invariants.
    if let Ok(first) = simulate_multi_stream::<STREAMS>(config) {
        let second = simulate_multi_stream::<STREAMS>(config)
            .expect("an accepted configuration must stay accepted");
        assert_eq!(
            first, second,
            "the shared-link simulation must be deterministic"
        );
        assert_scenario_invariants(&first, &config);
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same shared-link invariants.
    if data.len() >= 28 {
        let raw = MultiStreamScenarioConfig {
            frames: u32::from_le_bytes([data[14], data[15], data[16], data[17]]),
            frame_duration_ms: u64::from(u16::from_le_bytes([data[18], data[19]])),
            target_buffer_ms: u64::from(u16::from_le_bytes([data[20], data[21]])),
            media_datagram_bytes: u16::from_le_bytes([data[22], data[23]]),
            network: NetworkProfile::stable(u64::from(u32::from_le_bytes([
                data[24], data[25], data[26], data[27],
            ]))),
        };
        if let Ok(report) = simulate_multi_stream::<STREAMS>(raw) {
            assert_scenario_invariants(&report, &raw);
        }
    }
});

/// Asserts the shared-link invariants that hold for every accepted report.
fn assert_scenario_invariants(
    report: &MultiStreamSimulationReport<STREAMS>,
    config: &MultiStreamScenarioConfig,
) {
    assert_eq!(
        report.streams.len(),
        STREAMS,
        "one report per source stream"
    );
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames) * u64::try_from(STREAMS).unwrap(),
        "each stream must submit exactly one packet per frame on the shared link"
    );
    for stream in &report.streams {
        assert_eq!(
            stream.playout_frames + stream.plc_frames,
            config.frames,
            "every playout decision must play out or conceal exactly one frame"
        );
        assert!(
            stream.playout_frames <= config.frames,
            "a stream cannot play out more frames than it sends"
        );
        assert!(
            stream.plc_frames <= config.frames,
            "a stream cannot conceal more frames than it sends"
        );
    }
}

fn profile_from_bytes(data: &[u8]) -> NetworkProfile {
    let mut profile = NetworkProfile::stable(0x5A17_2026);
    profile.base_delay_ms = u64::from(data.get(3).copied().unwrap_or(0)) % 101;
    profile.jitter_ms = u64::from(data.get(4).copied().unwrap_or(0)) % 41;
    profile.reorder_delay_ms = u64::from(data.get(5).copied().unwrap_or(0)) % 101;
    profile.loss_percent = data.get(6).copied().unwrap_or(0) % 101;
    profile.duplicate_percent = data.get(7).copied().unwrap_or(0) % 101;
    profile.reorder_percent = data.get(8).copied().unwrap_or(0) % 101;
    // A zero interval disables bursts; otherwise the burst never overlaps its
    // interval so `NetworkProfile::validate` always accepts the pair.
    let burst_interval_packets = match data.get(9).copied().unwrap_or(0) % 8 {
        0 => 0_u32,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 12,
        5 => 16,
        6 => 20,
        _ => 32,
    };
    profile.burst_interval_packets = burst_interval_packets;
    profile.burst_length_packets = if burst_interval_packets == 0 {
        0
    } else {
        u16::try_from(u32::from(data.get(10).copied().unwrap_or(0)) % burst_interval_packets + 1)
            .expect("burst length fits u16")
    };
    profile
}
