//! Deterministic property coverage for the controller-driven membership
//! scenario.
//!
//! The controller-membership scenario must complete and stay bounded for any
//! valid configuration: every stream plays or conceals exactly its configured
//! frame count, the controller projection matches the mixer's actual slot
//! table after every render, published membership counters stay within their
//! churn bounds, seeded gain and mute scripts publish a bounded gain without
//! stalling playout or the bus, and every per-source quality ratio stays
//! within its bound. The same seeded profiles drive both ends of the Phase 2
//! range (two and four independent sources). This test drives seeded random
//! profiles, frame durations, and initial targets over a long run and asserts
//! those invariants on every report, mirroring the deterministic property
//! pattern used for the sibling scenarios and the
//! `mixer_controller_membership` fuzz target.

use sonimesh_network_simulator::{
    MixerControllerGainScript, MixerControllerScenarioConfig, NetworkProfile,
    simulate_mixer_controller_membership,
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
    // The ranges mirror the `mixer_controller_membership` fuzz target so the
    // seeded property run locally covers the same configuration space the
    // LibFuzzer smoke exercises on CI.
    network.loss_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 101)).unwrap();
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
        let frames = random.next_in(1, MAX_FRAMES + 1);
        let mut gain_scripts = [MixerControllerGainScript::default(); S];
        for script in &mut gain_scripts {
            if random.next_usize(2) == 0 {
                script.mute_at_frame = Some(random.next_in(0, frames));
            }
            if random.next_usize(2) == 0 {
                script.unmute_at_frame = Some(random.next_in(0, frames));
            }
            if random.next_usize(2) == 0 {
                script.gain_at_frame = Some(random.next_in(0, frames));
                script.gain_milli = u16::try_from(random.next_in(0, 1_001)).unwrap();
            }
        }
        let config = MixerControllerScenarioConfig::<S> {
            frames,
            frame_duration_ms: u64::from(frame_duration_ms),
            target_buffer_ms: u64::from(random.next_in(0, 1_001)),
            media_datagram_bytes: u16::try_from(
                random.next_in(1, u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap() + 1),
            )
            .unwrap(),
            network: random_profile(&mut random),
            gain_scripts,
        };

        let report = simulate_mixer_controller_membership::<S, SLOTS, PCM_BLOCK>(config)
            .expect("the seeded configuration must be valid");
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
        let expected_inserts = u32::try_from(S).expect("the Phase 2 stream count fits a u32");
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
}
