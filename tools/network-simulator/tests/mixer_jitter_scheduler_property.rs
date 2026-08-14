//! Deterministic property coverage for the combined jitter-scheduler and
//! mixer scenario.
//!
//! The shared-link scenario must complete and stay bounded for any valid
//! configuration: every stream plays or conceals exactly its configured frame
//! count, each stream's adaptive target remains inside its `[min, max]` water
//! levels, scheduler step counters never exceed the observation count, every
//! playout tick either consumes a frame or gates (so gated ticks plus frames
//! equal observations), every stream stays active in the fixed-slot mixer, and
//! the rendered bus stays finite. The same seeded profiles drive both ends of
//! the Phase 2 range (two and four independent sources). This test drives
//! seeded random profiles,
//! frame durations, delays, and gate deadlines over a long run and asserts
//! those invariants on every report, mirroring the deterministic property
//! pattern used for the sibling scheduler scenarios and the
//! `mixer_jitter_scheduler` fuzz target.

use sonimesh_audio::{JitterMode, JitterTargetConfig};
use sonimesh_network_simulator::{
    MixerJitterSchedulerScenarioConfig, NetworkProfile, simulate_mixer_jitter_scheduler,
};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    /// Returns a value in `low..high_exclusive` for `low < high_exclusive`.
    fn next_in(&mut self, low: u32, high_exclusive: u32) -> u32 {
        low + self.next_u32() % (high_exclusive - low)
    }
}

/// Jitter-buffer capacity shared by the scenario and its target presets.
const JITTER_BUFFER_CAPACITY: usize = 64;
/// Sender frames per stream stay small so every property step is bounded.
const MAX_FRAMES: u32 = 48;
/// Number of seeded configurations per property run.
const STEPS: usize = 256;
/// Fixed slot count of the mixer table (also the four-stream slot count).
const SLOTS: usize = 4;
/// Output-format PCM block size used by the mixer callback.
const PCM_BLOCK: usize = 64;

/// Builds a validated network profile within the scenario impairment range.
fn random_profile(random: &mut XorShift64) -> NetworkProfile {
    let mut network = NetworkProfile::stable(u64::from(random.next_u32()));
    network.loss_percent = u8::try_from(random.next_in(0, 31)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 11)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 9)).unwrap();
    network.base_delay_ms = u64::from(random.next_in(0, 101));
    network.jitter_ms = u64::from(random.next_in(0, 41));
    network.reorder_delay_ms = u64::from(random.next_in(0, 101));
    // A zero interval disables bursts; otherwise the burst never overlaps its
    // interval so `NetworkProfile::validate` always accepts the pair.
    let burst_interval_packets = match random.next_usize(8) {
        0 => 0_u32,
        1 => 2,
        2 => 4,
        3 => 8,
        4 => 12,
        5 => 16,
        6 => 20,
        _ => 32,
    };
    network.burst_interval_packets = burst_interval_packets;
    network.burst_length_packets = if burst_interval_packets == 0 {
        0
    } else {
        u16::try_from(random.next_in(1, burst_interval_packets + 1)).unwrap()
    };
    network
}

#[test]
fn every_valid_configuration_stays_bounded_and_consumes_every_frame() {
    // Phase 2 admits two to four independent sources; verify both ends of the
    // range over the same seeded profiles.
    check_stream_count::<2>();
    check_stream_count::<4>();
}

/// Drives `STEPS` seeded configurations at a fixed stream count and asserts
/// the scenario invariants on every report.
fn check_stream_count<const S: usize>() {
    let mut random = XorShift64(0x5A17_2026);
    for _ in 0..STEPS {
        let frame_duration_ms = if random.next_usize(2) == 0 {
            10_u32
        } else {
            20_u32
        };
        let mode = match random.next_usize(3) {
            0 => JitterMode::LowLatency,
            1 => JitterMode::Stable,
            _ => JitterMode::Balanced,
        };
        let config = MixerJitterSchedulerScenarioConfig::<S> {
            frames: random.next_in(1, MAX_FRAMES + 1),
            frame_duration_ms: u64::from(frame_duration_ms),
            initial_playout_ms: u64::from(random.next_in(0, 1_001)),
            media_datagram_bytes: u16::try_from(
                random.next_in(1, u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap() + 1),
            )
            .unwrap(),
            network: random_profile(&mut random),
            target: JitterTargetConfig::preset(mode, frame_duration_ms, JITTER_BUFFER_CAPACITY),
            max_gate_ticks: random.next_in(1, 65),
        };

        let report = simulate_mixer_jitter_scheduler::<S, SLOTS, PCM_BLOCK>(config)
            .expect("the seeded configuration must be valid");
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
        }
        assert_eq!(
            report.mixer.active_streams, S,
            "every stream must stay active in the fixed-slot mixer"
        );
        assert!(report.mixer.renders > 0, "the callback must render the bus");
        assert!(report.mixer.output_finite, "the bus must stay finite");
    }
}
