#![no_main]

//! Fuzz the target-gated playout scheduler scenario.
//!
//! Arbitrary bounded scenario configurations drive `simulate_jitter_scheduler`
//! end to end: one source stream traverses an impaired virtual link while a
//! per-stream ADR 0016 adaptive target gates playout, waiting while fewer than
//! the target encoded frames are buffered and forcing a playout once the gate
//! deadline expires so a mid-stream stall can never become unbounded. An
//! accepted configuration must consume every frame through playout or PLC,
//! count each scheduler tick as one gated wait or one consumed frame, keep the
//! adaptive target inside its bounded `[min, max]` water levels, keep forced
//! playouts and step counters within their bounds, and complete inside the
//! scenario's explicit hard deadline while staying deterministic; a rejected
//! configuration (including an expired hard deadline) must return a clean
//! scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    JitterSchedulerScenarioConfig, JitterSchedulerSimulationReport, NetworkProfile,
    simulate_jitter_scheduler,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Encoded-frame capacity shared by the scenario and its target preset.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Sender frames stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 200;
/// Initial scheduler tick is bounded by the scenario validation upper bound.
const MAX_INITIAL_PLAYOUT_MS: u64 = 1_000;
/// Gate deadline is bounded by the scenario hard bound.
const MAX_GATE_TICKS: u32 = 64;

fuzz_target!(|data: &[u8]| {
    let frame_duration_ms = if data.first().is_some_and(|byte| byte % 2 == 0) {
        10_u32
    } else {
        20_u32
    };
    let mode = match data.get(1).map_or(0, |byte| byte % 3) {
        0 => JitterMode::LowLatency,
        1 => JitterMode::Stable,
        _ => JitterMode::Balanced,
    };
    let frames = u32::from(u16::from_le_bytes([
        data.get(2).copied().unwrap_or(0),
        data.get(3).copied().unwrap_or(0),
    ])) % MAX_FUZZ_FRAMES
        + 1;
    let network = profile_from_bytes(data);
    let config = JitterSchedulerScenarioConfig {
        frames,
        frame_duration_ms: u64::from(frame_duration_ms),
        initial_playout_ms: u64::from(data.get(4).copied().unwrap_or(0))
            % (MAX_INITIAL_PLAYOUT_MS + 1),
        media_datagram_bytes: u16::try_from(
            usize::from(data.get(5).copied().unwrap_or(0)) % MAX_MEDIA_DATAGRAM_LEN + 1,
        )
        .expect("media datagram bound fits u16"),
        network,
        target: JitterTargetConfig::preset(mode, frame_duration_ms, JITTER_BUFFER_CAPACITY),
        max_gate_ticks: u32::from(data.get(6).copied().unwrap_or(1)) % MAX_GATE_TICKS + 1,
    };

    // A configuration whose hard deadline elapses before every frame is
    // consumed returns a clean error; accepted configurations must satisfy the
    // invariants.
    if let Ok(first) = simulate_jitter_scheduler(config) {
        let second = simulate_jitter_scheduler(config)
            .expect("an accepted configuration must stay accepted");
        assert_eq!(
            first, second,
            "the scheduler simulation must be deterministic"
        );
        assert_scenario_invariants(&first, &config);
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same invariants.
    if data.len() >= 32 {
        let raw_frame_duration_ms = u32::from(u16::from_le_bytes([data[16], data[17]]));
        let raw = JitterSchedulerScenarioConfig {
            frames: u32::from_le_bytes([data[18], data[19], data[20], data[21]]),
            frame_duration_ms: u64::from(raw_frame_duration_ms),
            initial_playout_ms: u64::from(u16::from_le_bytes([data[22], data[23]])),
            media_datagram_bytes: u16::from_le_bytes([data[24], data[25]]),
            network: NetworkProfile::stable(u64::from(u32::from_le_bytes([
                data[26], data[27], data[28], data[29],
            ]))),
            target: JitterTargetConfig::preset(
                mode_from_byte(data.get(30).copied().unwrap_or(0)),
                raw_frame_duration_ms,
                JITTER_BUFFER_CAPACITY,
            ),
            max_gate_ticks: u32::from(data.get(31).copied().unwrap_or(1)),
        };
        if let Ok(report) = simulate_jitter_scheduler(raw) {
            assert_scenario_invariants(&report, &raw);
        }
    }
});

/// Asserts the scheduler invariants that hold for every accepted report.
fn assert_scenario_invariants(
    report: &JitterSchedulerSimulationReport,
    config: &JitterSchedulerScenarioConfig,
) {
    let consumed = report.playout.playout_frames + report.playout.plc_frames;
    assert_eq!(
        consumed, config.frames,
        "every configured frame must be played or concealed"
    );
    assert_eq!(
        report.gated_ticks + consumed,
        report.observations,
        "each scheduler tick is one gated wait or one consumed frame"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.final_target_frames),
        "final target must stay inside the bounded range"
    );
    assert!(
        (report.min_frames..=report.max_frames).contains(&report.base_frames),
        "mode base must stay inside the bounded range"
    );
    assert!(
        report.raised_steps + report.lowered_steps <= report.observations,
        "step counters must not exceed the observation count"
    );
    assert!(
        report.forced_playouts <= config.frames,
        "forced playouts must not exceed the configured frame count"
    );
    assert!(
        report.final_buffered_frames <= JITTER_BUFFER_CAPACITY,
        "final buffered frames must not exceed the jitter-buffer capacity"
    );

    // Completion latency is bounded by the scenario's explicit hard deadline:
    // every frame may wait at most (max_gate_ticks + 2) scheduler periods plus
    // the worst-case one-way network delay before a forced playout.
    let hard_slack_ms = u64::from(config.frames)
        .saturating_mul(u64::from(config.max_gate_ticks + 2))
        .saturating_mul(config.frame_duration_ms)
        .saturating_add(config.network.base_delay_ms)
        .saturating_add(config.network.jitter_ms)
        .saturating_add(config.network.reorder_delay_ms)
        .saturating_add(2);
    assert!(
        report.excess_latency_ms <= hard_slack_ms,
        "completion must never exceed the hard deadline"
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
    profile
}
