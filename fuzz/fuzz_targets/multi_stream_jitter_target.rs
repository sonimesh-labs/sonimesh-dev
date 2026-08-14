#![no_main]

//! Fuzz the multi-stream adaptive-jitter-target scenario.
//!
//! Arbitrary bounded scenario configurations drive
//! `simulate_multi_stream_jitter_target` end to end: two source streams share
//! one impaired virtual link while each stream owns an independent ADR 0016
//! adaptive target that observes that stream's arrival jitter and encoded-frame
//! water level once per playout decision. An accepted configuration must submit
//! exactly one packet per stream per frame on the shared link, consume every
//! frame through playout or PLC, observe once per frame, keep every adaptive
//! target inside its bounded `[min, max]` water levels, keep step counters
//! below the observation count, and stay deterministic; a rejected
//! configuration must return a clean scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    MultiStreamJitterTargetScenarioConfig, MultiStreamJitterTargetSimulationReport, NetworkProfile,
    NetworkStats, simulate_multi_stream_jitter_target,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Two source streams share one virtual link.
const STREAMS: usize = 2;
/// Encoded-frame capacity shared by the scenario and its target presets.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Sender frames per stream stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 200;

fuzz_target!(|data: &[u8]| {
    let frame_duration_ms = if data.first().is_some_and(|byte| byte % 2 == 0) {
        10_u32
    } else {
        20_u32
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
    let targets = std::array::from_fn(|stream| {
        let mode = match data.get(14 + stream).map_or(0, |byte| byte % 3) {
            0 => JitterMode::LowLatency,
            1 => JitterMode::Stable,
            _ => JitterMode::Balanced,
        };
        JitterTargetConfig::preset(mode, frame_duration_ms, JITTER_BUFFER_CAPACITY)
    });
    let config = MultiStreamJitterTargetScenarioConfig {
        frames,
        frame_duration_ms: u64::from(frame_duration_ms),
        target_buffer_ms,
        media_datagram_bytes,
        network,
        targets,
    };

    // Playout is pre-armed at the negotiated start sequence, so a lost or
    // late first packet conceals via PLC rather than reporting NotReady;
    // accepted configurations must satisfy the invariants.
    if let Ok(first) = simulate_multi_stream_jitter_target::<STREAMS>(config) {
        let second = simulate_multi_stream_jitter_target::<STREAMS>(config)
            .expect("an accepted configuration must stay accepted");
        assert_eq!(
            first, second,
            "the shared-link simulation must be deterministic"
        );
        assert_scenario_invariants(&first, &config);
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same invariants.
    if data.len() >= 32 {
        let raw_frame_duration_ms = u32::from(u16::from_le_bytes([data[16], data[17]]));
        let raw = MultiStreamJitterTargetScenarioConfig {
            frames: u32::from_le_bytes([data[18], data[19], data[20], data[21]]),
            frame_duration_ms: u64::from(raw_frame_duration_ms),
            target_buffer_ms: u64::from(u16::from_le_bytes([data[22], data[23]])),
            media_datagram_bytes: u16::from_le_bytes([data[24], data[25]]),
            network: NetworkProfile::stable(u64::from(u32::from_le_bytes([
                data[26], data[27], data[28], data[29],
            ]))),
            targets: std::array::from_fn(|stream| {
                let mode = match data.get(30 + stream).map_or(0, |byte| byte % 3) {
                    0 => JitterMode::LowLatency,
                    1 => JitterMode::Stable,
                    _ => JitterMode::Balanced,
                };
                JitterTargetConfig::preset(mode, raw_frame_duration_ms, JITTER_BUFFER_CAPACITY)
            }),
        };
        if let Ok(report) = simulate_multi_stream_jitter_target::<STREAMS>(raw) {
            assert_scenario_invariants(&report, &raw);
        }
    }
});

/// Asserts the shared-link and per-stream invariants that hold for every
/// accepted report.
fn assert_scenario_invariants(
    report: &MultiStreamJitterTargetSimulationReport<STREAMS>,
    config: &MultiStreamJitterTargetScenarioConfig<STREAMS>,
) {
    assert_network_accounting(
        &report.network,
        &config.network,
        config.media_datagram_bytes,
    );
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames) * u64::try_from(STREAMS).unwrap(),
        "each stream must submit exactly one packet per frame on the shared link"
    );
    for (index, stream) in report.streams.iter().enumerate() {
        let consumed = stream.playout.playout_frames + stream.playout.plc_frames;
        assert_eq!(
            consumed, config.frames,
            "stream {index} must play or conceal every configured frame"
        );
        assert_eq!(
            stream.observations, config.frames,
            "stream {index} must observe once per configured frame"
        );
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
            "stream {index} target must stay inside the bounded range"
        );
        assert!(
            (stream.min_frames..=stream.max_frames).contains(&stream.base_frames),
            "stream {index} mode base must stay inside the bounded range"
        );
        assert!(
            stream.raised_steps + stream.lowered_steps <= stream.observations,
            "stream {index} steps must not exceed observations"
        );
        assert!(
            stream.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
            "stream {index} buffered frames must not exceed the jitter capacity"
        );
    }
}

/// Asserts the finite-link serialization and sender-side backpressure
/// accounting that must hold on every accepted report, mirroring the seeded
/// property walks: every scheduled copy is serialized as exactly one media
/// datagram, and without a finite link or a socket buffer the sender never
/// reports backpressure.
fn assert_network_accounting(
    stats: &NetworkStats,
    profile: &NetworkProfile,
    media_datagram_bytes: u16,
) {
    let bandwidth = profile.bandwidth_bytes_per_second;
    if bandwidth == 0 {
        assert_eq!(
            stats.bandwidth_shaped_copies, 0,
            "an unshaped link must not serialize any copy"
        );
        assert_eq!(
            stats.bandwidth_serialized_bytes, 0,
            "an unshaped link must not place bytes on a link"
        );
    } else {
        assert_eq!(
            stats.bandwidth_shaped_copies, stats.scheduled_copies,
            "every scheduled copy is serialized on the finite link"
        );
        assert_eq!(
            stats.bandwidth_serialized_bytes,
            stats.scheduled_copies * u64::from(media_datagram_bytes),
            "serialized bytes equal one datagram per scheduled copy"
        );
    }

    if profile.send_buffer_bytes.is_none() || bandwidth == 0 {
        assert_eq!(
            stats.send_buffer_full_packets, 0,
            "no finite link or no socket buffer means no backpressure"
        );
        assert_eq!(
            stats.send_buffer_queued_bytes, 0,
            "no finite link or no socket buffer means no queued bytes"
        );
    } else if let Some(capacity) = profile.send_buffer_bytes {
        assert!(
            stats.send_buffer_queued_bytes <= capacity,
            "sender-side queued bytes never exceed the configured socket-buffer capacity"
        );
    }
}

fn profile_from_bytes(data: &[u8]) -> NetworkProfile {
    let mut profile = NetworkProfile::stable(0x5A17_2026);
    profile.base_delay_ms = u64::from(data.get(3).copied().unwrap_or(0)) % 101;
    profile.jitter_ms = u64::from(data.get(4).copied().unwrap_or(0)) % 41;
    profile.reorder_delay_ms = u64::from(data.get(5).copied().unwrap_or(0)) % 101;
    profile.loss_percent = data.get(6).copied().unwrap_or(0) % 16;
    profile.duplicate_percent = data.get(7).copied().unwrap_or(0) % 11;
    profile.reorder_percent = data.get(8).copied().unwrap_or(0) % 9;
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
    // Occasionally shape the shared link and bound the sender-side socket
    // buffer so fuzz inputs exercise finite-link serialization and
    // backpressure, mirroring the seeded property walks. The reads use fixed
    // high offsets that no other field in this target consumes.
    let shaped = data.get(40).copied().unwrap_or(0) % 100 >= 10;
    let bw_mode = data.get(41).copied().unwrap_or(0) % 100;
    let bandwidth_raw = u16::from_le_bytes([
        data.get(42).copied().unwrap_or(0),
        data.get(43).copied().unwrap_or(0),
    ]);
    profile.bandwidth_bytes_per_second = if !shaped {
        0
    } else if bw_mode < 30 {
        u64::from(bandwidth_raw % 4_801 + 200) // 200..=5_000 bytes/s
    } else {
        u64::from(bandwidth_raw) * 30 + 5_000 // 5_000..=~1_971_050 bytes/s
    };
    profile.max_in_flight_packets = usize::from(data.get(44).copied().unwrap_or(0) % 15 + 2);
    let buffer_sel = data.get(45).copied().unwrap_or(0) % 100;
    profile.send_buffer_bytes = if !shaped || buffer_sel < 30 {
        None
    } else if buffer_sel < 50 {
        Some(u64::from(
            u32::from(data.get(46).copied().unwrap_or(0)) % 9_601 + 1,
        ))
    } else {
        let capacity = u32::from_le_bytes([
            data.get(47).copied().unwrap_or(0),
            data.get(48).copied().unwrap_or(0),
            data.get(49).copied().unwrap_or(0),
            data.get(50).copied().unwrap_or(0),
        ]);
        Some(u64::from(capacity % 2_000_000 + 1))
    };
    profile
}
