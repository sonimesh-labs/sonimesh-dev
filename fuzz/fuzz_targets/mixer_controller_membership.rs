#![no_main]

//! Fuzz the controller-driven mixer membership scenario.
//!
//! Arbitrary bounded scenario configurations drive
//! `simulate_mixer_controller_membership` end to end: each stream's sender
//! emits frames over one shared impaired link, every stream is admitted
//! through the real `MixerMembershipController` into a fixed-slot
//! `CommandDrivenMixer`, the callback renders the bus every tick, and a
//! mid-session remove/re-admit exercises membership churn. An accepted
//! configuration must consume every frame through playout or PLC, publish
//! bounded per-stream gain and mute events, keep every per-source quality
//! ratio within its bound, keep the controller projection matched to the
//! mixer's actual slot table, and render a finite bus; a rejected
//! configuration must return a clean scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_network_simulator::{
    MixerControllerGainScript, MixerControllerScenarioConfig, NetworkProfile, NetworkStats,
    simulate_mixer_controller_membership,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

const STREAMS: usize = 2;
const SLOTS: usize = 4;
const PCM_BLOCK: usize = 64;
/// Sender frames per stream stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 48;
/// Derived delays stay far below the profile hard bounds to keep runs fast.
const MAX_BASE_DELAY_MS: u64 = 100;
const MAX_JITTER_MS: u64 = 40;
const MAX_REORDER_DELAY_MS: u64 = 100;
/// Initial playout target is bounded by the scenario validation upper bound.
const MAX_TARGET_BUFFER_MS: u64 = 1_000;

fuzz_target!(|data: &[u8]| {
    let frame_duration_ms = if data.first().is_some_and(|byte| byte % 2 == 0) {
        10_u32
    } else {
        20_u32
    };
    let frames = u32::from(data.get(2).copied().unwrap_or(0)) % MAX_FUZZ_FRAMES + 1;
    let gain_scripts = [MixerControllerGainScript {
        mute_at_frame: data
            .get(14)
            .and_then(|byte| (*byte != 0).then_some(u32::from(*byte) % frames)),
        unmute_at_frame: data
            .get(15)
            .and_then(|byte| (*byte != 0).then_some(u32::from(*byte) % frames)),
        gain_at_frame: data
            .get(16)
            .and_then(|byte| (*byte != 0).then_some(u32::from(*byte) % frames)),
        gain_milli: u16::try_from(u32::from(data.get(17).copied().unwrap_or(0)) % 1_001)
            .expect("gain milli fits u16"),
    }; STREAMS];
    let config = MixerControllerScenarioConfig {
        frames,
        frame_duration_ms: u64::from(frame_duration_ms),
        target_buffer_ms: u64::from(data.get(3).copied().unwrap_or(0)) % (MAX_TARGET_BUFFER_MS + 1),
        media_datagram_bytes: u16::try_from(
            usize::from(data.get(4).copied().unwrap_or(0)) % MAX_MEDIA_DATAGRAM_LEN + 1,
        )
        .expect("media datagram bound fits u16"),
        network: profile_from_bytes(data),
        gain_scripts,
    };

    if let Ok(report) = simulate_mixer_controller_membership::<STREAMS, SLOTS, PCM_BLOCK>(config) {
        assert_network_accounting(
            &report.network,
            &config.network,
            config.media_datagram_bytes,
        );
        for stream in &report.streams {
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames,
                "every frame must be consumed by playout or PLC"
            );
            assert!(
                stream.gain_changes <= 3,
                "at most one mute, one unmute, and one gain event per stream"
            );
            assert!(
                stream.final_gain_milli <= 1_000,
                "published gain must stay within the bounded linear range"
            );
            assert!(
                stream.quality.within_bounds(),
                "every per-source quality ratio must stay within its bound"
            );
        }
        assert!(
            report.mixer.projection_matched,
            "the controller projection must match the mixer after every render"
        );
        assert_eq!(
            report.mixer.active_streams, report.mixer.projected_streams,
            "the final active counts must agree"
        );
        let expected_inserts = u32::try_from(STREAMS).expect("the stream count fits a u32");
        assert!(
            report.mixer.published_inserts >= expected_inserts,
            "every initial admission must be published"
        );
        assert!(
            report.mixer.published_inserts <= expected_inserts + 1,
            "only the one churn re-admission can add an insert"
        );
        assert_eq!(
            report.mixer.published_removes, 1,
            "the mid-session churn removes exactly stream zero"
        );
        assert!(report.mixer.renders > 0, "the callback must render the bus");
        assert!(report.mixer.output_finite, "the bus must stay finite");
    }
});

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
    profile.base_delay_ms = u64::from(data.get(6).copied().unwrap_or(0)) % (MAX_BASE_DELAY_MS + 1);
    profile.jitter_ms = u64::from(data.get(7).copied().unwrap_or(0)) % (MAX_JITTER_MS + 1);
    profile.reorder_delay_ms =
        u64::from(data.get(8).copied().unwrap_or(0)) % (MAX_REORDER_DELAY_MS + 1);
    profile.loss_percent = data.get(9).copied().unwrap_or(0) % 101;
    profile.duplicate_percent = data.get(10).copied().unwrap_or(0) % 101;
    profile.reorder_percent = data.get(11).copied().unwrap_or(0) % 101;
    // A zero interval disables bursts; otherwise the burst never overlaps its
    // interval so `NetworkProfile::validate` always accepts the pair.
    let burst_interval_packets = match data.get(12).copied().unwrap_or(0) % 8 {
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
        u16::try_from(u32::from(data.get(13).copied().unwrap_or(0)) % burst_interval_packets + 1)
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
