#![no_main]

//! Fuzz the fixed-slot mixer membership scenario over a shared link.
//!
//! Arbitrary bounded scenario configurations drive
//! `simulate_mixer_membership` end to end: two source streams share one
//! impaired virtual link, every stream's playout is decoded to a finite PCM
//! block pushed into its slot of the fixed-slot ADR 0015 `CommandDrivenMixer`,
//! the callback renders the bus every tick, and a mid-session
//! remove/reinsert exercises membership churn. An accepted configuration must
//! consume every frame through playout or PLC, submit exactly one packet per
//! stream per frame, keep both streams active after the reinsert, publish the
//! two starts plus one reinsert and one remove, and render a finite bus while
//! staying deterministic; a rejected configuration must return a clean
//! scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_network_simulator::{
    MixerMembershipScenarioConfig, MixerMembershipSimulationReport, NetworkProfile,
    simulate_mixer_membership,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Two source streams share one link and the mixer table.
const STREAMS: usize = 2;
/// Fixed slot count of the mixer membership table.
const SLOTS: usize = 4;
/// Output-format PCM block size used by the mixer callback.
const PCM_BLOCK: usize = 64;
/// Sender frames per stream are large enough that the mid-session churn and
/// reinsert always complete inside the simulation window.
const MIN_FUZZ_FRAMES: u32 = 30;
const MAX_FUZZ_FRAMES: u32 = 200;
/// Initial playout target is bounded by the scenario validation upper bound.
const MAX_TARGET_BUFFER_MS: u64 = 1_000;

fuzz_target!(|data: &[u8]| {
    let frame_duration_ms = if data.first().is_some_and(|byte| byte % 2 == 0) {
        10_u64
    } else {
        20_u64
    };
    let frames = u32::from(u16::from_le_bytes([
        data.get(1).copied().unwrap_or(0),
        data.get(2).copied().unwrap_or(0),
    ])) % (MAX_FUZZ_FRAMES - MIN_FUZZ_FRAMES + 1)
        + MIN_FUZZ_FRAMES;
    let network = profile_from_bytes(data);
    let config = MixerMembershipScenarioConfig {
        frames,
        frame_duration_ms,
        target_buffer_ms: u64::from(u16::from_le_bytes([
            data.get(3).copied().unwrap_or(0),
            data.get(4).copied().unwrap_or(0),
        ])) % (MAX_TARGET_BUFFER_MS + 1),
        media_datagram_bytes: u16::try_from(
            usize::from(data.get(5).copied().unwrap_or(0)) % MAX_MEDIA_DATAGRAM_LEN + 1,
        )
        .expect("media datagram bound fits u16"),
        network,
    };

    if let Ok(first) = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config) {
        let second = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config)
            .expect("an accepted configuration must stay accepted");
        assert_eq!(first, second, "the simulation must be deterministic");
        assert_scenario_invariants(&first, &config, true);
    }

    // Arbitrary raw scenario fields must not panic validation; accepted ones
    // must satisfy the same transport and bus invariants. A small raw frame
    // count can end before the reinsert, so only the always-true invariants
    // are asserted there.
    if data.len() >= 30 {
        let raw = MixerMembershipScenarioConfig {
            frames: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
            frame_duration_ms: u64::from(u16::from_le_bytes([data[20], data[21]])),
            target_buffer_ms: u64::from(u16::from_le_bytes([data[22], data[23]])),
            media_datagram_bytes: u16::from_le_bytes([data[24], data[25]]),
            network: NetworkProfile::stable(u64::from(u32::from_le_bytes([
                data[26], data[27], data[28], data[29],
            ]))),
        };
        if let Ok(report) = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(raw) {
            assert_scenario_invariants(&report, &raw, false);
        }
    }
});

/// Asserts the transport, membership, and bus invariants that hold for every
/// accepted report.
fn assert_scenario_invariants(
    report: &MixerMembershipSimulationReport<STREAMS, SLOTS>,
    config: &MixerMembershipScenarioConfig<STREAMS>,
    reinsert_completed: bool,
) {
    assert_eq!(
        report.network.submitted_packets,
        u64::from(config.frames) * u64::try_from(STREAMS).unwrap(),
        "each stream must submit exactly one packet per frame"
    );
    for (index, stream) in report.streams.iter().enumerate() {
        assert_eq!(
            stream.playout_frames + stream.plc_frames,
            config.frames,
            "stream {index} must play or conceal every configured frame"
        );
    }
    assert_eq!(
        report.mixer.slots, SLOTS,
        "the report must expose the fixed slot count"
    );
    assert_eq!(
        report.mixer.published_removes, 1,
        "the mid-session remove always completes"
    );
    if reinsert_completed {
        assert_eq!(
            report.mixer.active_streams, STREAMS,
            "both streams must be active after the reinsert"
        );
        assert_eq!(
            report.mixer.published_inserts, 3,
            "two starts plus the mid-session reinsert"
        );
    } else {
        assert!(
            (1..=STREAMS).contains(&report.mixer.active_streams),
            "the active count must stay within the stream bound"
        );
        assert!(
            (2..=3).contains(&report.mixer.published_inserts),
            "the starts plus an optional reinsert"
        );
    }
    assert!(report.mixer.renders > 0, "the callback must render the bus");
    assert!(report.mixer.output_finite, "the bus must stay finite");
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
