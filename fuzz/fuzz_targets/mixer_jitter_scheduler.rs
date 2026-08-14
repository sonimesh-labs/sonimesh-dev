#![no_main]

//! Fuzz the combined target-gated playout and fixed-slot mixer scenario.
//!
//! Arbitrary bounded scenario configurations drive
//! `simulate_mixer_jitter_scheduler` end to end: each stream's sender emits
//! frames over one shared impaired link, the per-stream adaptive target
//! (ADR 0016) gates playout and forces a playout once the gate deadline
//! expires, and every played-out frame is decoded to a finite PCM block
//! pushed into that stream's slot of the fixed-slot mixer (ADR 0015), whose
//! callback renders the bus every tick. An accepted configuration must
//! consume every frame through playout or PLC, keep every adaptive target
//! inside its bounded water levels, keep scheduler step counters below the
//! observation count, keep every stream active in the mixer, publish bounded
//! per-stream gain and mute events, and render a finite bus; a rejected
//! configuration must return a clean scenario error rather than panic.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    MixerJitterGainScript, MixerJitterSchedulerScenarioConfig, NetworkProfile,
    simulate_mixer_jitter_scheduler,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

const STREAMS: usize = 2;
const SLOTS: usize = 4;
const PCM_BLOCK: usize = 64;
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Sender frames per stream stay small so every fuzz run is bounded.
const MAX_FUZZ_FRAMES: u32 = 48;
/// Derived delays stay far below the profile hard bounds to keep runs fast.
const MAX_BASE_DELAY_MS: u64 = 100;
const MAX_JITTER_MS: u64 = 40;
const MAX_REORDER_DELAY_MS: u64 = 100;
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
    let frames = u32::from(data.get(2).copied().unwrap_or(0)) % MAX_FUZZ_FRAMES + 1;
    let gain_scripts = [MixerJitterGainScript {
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
    let config = MixerJitterSchedulerScenarioConfig {
        frames,
        frame_duration_ms: u64::from(frame_duration_ms),
        initial_playout_ms: u64::from(data.get(3).copied().unwrap_or(0))
            % (MAX_INITIAL_PLAYOUT_MS + 1),
        media_datagram_bytes: u16::try_from(
            usize::from(data.get(4).copied().unwrap_or(0)) % MAX_MEDIA_DATAGRAM_LEN + 1,
        )
        .expect("media datagram bound fits u16"),
        network: profile_from_bytes(data),
        target: JitterTargetConfig::preset(mode, frame_duration_ms, JITTER_BUFFER_CAPACITY),
        max_gate_ticks: u32::from(data.get(5).copied().unwrap_or(1)) % MAX_GATE_TICKS + 1,
        gain_scripts,
    };

    if let Ok(report) = simulate_mixer_jitter_scheduler::<STREAMS, SLOTS, PCM_BLOCK>(config) {
        for stream in &report.streams {
            assert_eq!(
                stream.playout.playout_frames + stream.playout.plc_frames,
                config.frames,
                "every frame must be consumed by playout or PLC"
            );
            assert!(
                (stream.min_frames..=stream.max_frames).contains(&stream.final_target_frames),
                "the adaptive target must stay within its explicit bounds"
            );
            assert!(
                stream.raised_steps + stream.lowered_steps <= stream.observations,
                "step counters must not exceed the observation count"
            );
            assert_eq!(
                stream.gated_ticks + config.frames,
                stream.observations,
                "every playout tick either consumed a frame or gated"
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
        assert_eq!(
            report.mixer.active_streams, STREAMS,
            "both streams must stay active in the fixed-slot mixer"
        );
        assert!(report.mixer.renders > 0, "the callback must render the bus");
        assert!(report.mixer.output_finite, "the bus must stay finite");
    }
});

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
    profile
}
