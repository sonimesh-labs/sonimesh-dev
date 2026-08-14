#![no_main]

//! Fuzz the single-stream adaptive-jitter-target scenario.
//!
//! Arbitrary bounded scenario configurations drive `simulate_jitter_target`
//! end to end: one source stream traverses an impaired virtual link while the
//! ADR 0016 adaptive target observes that stream's arrival-jitter deviation
//! and encoded-frame water level once per playout decision. An accepted
//! configuration must submit exactly one packet per frame, consume every
//! frame through playout or PLC, observe once per frame, keep the adaptive
//! target inside its bounded `[min, max]` water levels, keep step counters
//! below the observation count, and stay deterministic; a rejected
//! configuration must return a clean scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterTargetScenarioConfig, JitterTargetSimulationReport, NetworkProfile,
    simulate_jitter_target,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Encoded-frame capacity shared by the scenario and its target preset.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Sender frames stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 200;
/// Initial playout target is bounded by the scenario validation upper bound.
const MAX_TARGET_BUFFER_MS: u64 = 1_000;

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
    let config = JitterTargetScenarioConfig {
        frames,
        frame_duration_ms: u64::from(frame_duration_ms),
        // The scenario pre-arms playout at the negotiated start sequence, so
        // the full valid initial-target range is accepted: a playout before
        // the first frame arrives conceals instead of rejecting.
        target_buffer_ms: u64::from(u16::from_le_bytes([
            data.get(3).copied().unwrap_or(0),
            data.get(4).copied().unwrap_or(0),
        ])) % (MAX_TARGET_BUFFER_MS + 1),
        media_datagram_bytes: u16::try_from(
            usize::from(data.get(5).copied().unwrap_or(0)) % MAX_MEDIA_DATAGRAM_LEN + 1,
        )
        .expect("media datagram bound fits u16"),
        network,
        target: JitterTargetConfig::preset(
            mode_from_byte(data.get(6).copied().unwrap_or(0)),
            frame_duration_ms,
            JITTER_BUFFER_CAPACITY,
        ),
    };

    if let Ok(first) = simulate_jitter_target(config) {
        let second =
            simulate_jitter_target(config).expect("an accepted configuration must stay accepted");
        assert_eq!(first, second, "the simulation must be deterministic");
        assert_scenario_invariants(&first, &config);
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same invariants.
    if data.len() >= 31 {
        let raw_frame_duration_ms = u32::from(u16::from_le_bytes([data[16], data[17]]));
        let raw = JitterTargetScenarioConfig {
            frames: u32::from_le_bytes([data[18], data[19], data[20], data[21]]),
            frame_duration_ms: u64::from(raw_frame_duration_ms),
            target_buffer_ms: u64::from(u16::from_le_bytes([data[22], data[23]])),
            media_datagram_bytes: u16::from_le_bytes([data[24], data[25]]),
            network: NetworkProfile::stable(u64::from(u32::from_le_bytes([
                data[26], data[27], data[28], data[29],
            ]))),
            target: JitterTargetConfig::preset(
                mode_from_byte(data.get(30).copied().unwrap_or(0)),
                raw_frame_duration_ms,
                JITTER_BUFFER_CAPACITY,
            ),
        };
        if let Ok(report) = simulate_jitter_target(raw) {
            assert_scenario_invariants(&report, &raw);
        }
    }
});

/// Asserts the single-stream invariants that hold for every accepted report.
fn assert_scenario_invariants(
    report: &JitterTargetSimulationReport,
    config: &JitterTargetScenarioConfig,
) {
    let consumed = report.playout.playout_frames + report.playout.plc_frames;
    assert_eq!(
        consumed, config.frames,
        "every configured frame must be played or concealed"
    );
    assert_eq!(
        report.observations, config.frames,
        "the controller must observe once per configured frame"
    );
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames),
        "exactly one packet must be submitted per frame"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "target must stay inside the bounded range"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.base_frames),
        "mode base must stay inside the bounded range"
    );
    assert!(
        report.raised_steps + report.lowered_steps <= report.observations,
        "steps must not exceed observations"
    );
    assert!(
        report.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
        "buffered frames must not exceed the jitter capacity"
    );
}

fn mode_from_byte(byte: u8) -> JitterMode {
    match byte % 3 {
        0 => JitterMode::LowLatency,
        1 => JitterMode::Stable,
        _ => JitterMode::Balanced,
    }
}

fn profile_from_bytes(data: &[u8]) -> NetworkProfile {
    let mut profile = NetworkProfile::stable(0x5A17_2026);
    profile.base_delay_ms = u64::from(data.get(7).copied().unwrap_or(0)) % 101;
    profile.jitter_ms = u64::from(data.get(8).copied().unwrap_or(0)) % 41;
    profile.reorder_delay_ms = u64::from(data.get(9).copied().unwrap_or(0)) % 101;
    profile.loss_percent = data.get(10).copied().unwrap_or(0) % 16;
    profile.duplicate_percent = data.get(11).copied().unwrap_or(0) % 11;
    profile.reorder_percent = data.get(12).copied().unwrap_or(0) % 9;
    // A zero interval disables bursts; otherwise the burst never overlaps its
    // interval so `NetworkProfile::validate` always accepts the pair.
    let burst_interval_packets = match data.get(13).copied().unwrap_or(0) % 8 {
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
        u16::try_from(u32::from(data.get(14).copied().unwrap_or(0)) % burst_interval_packets + 1)
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
