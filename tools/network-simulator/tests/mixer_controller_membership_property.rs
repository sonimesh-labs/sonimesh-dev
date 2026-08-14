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
//!
//! Profiles also vary the media datagram size and occasionally configure a
//! finite serial link and a bounded sender-side socket buffer shared by all
//! streams, so the walk pins the bandwidth-serialization accounting (every
//! scheduled copy is serialized as one datagram) and the no-link/no-buffer
//! backpressure semantics.

use sonimesh_network_simulator::{
    MixerControllerGainScript, MixerControllerScenarioConfig, MixerControllerSimulationReport,
    NetworkProfile, simulate_mixer_controller_membership,
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

    /// Returns a value in `0..=100`.
    fn percent(&mut self) -> usize {
        self.next_usize(101)
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
///
/// The media datagram size is passed in so a tight sender-side socket buffer
/// can be sized relative to the datagram the scenario actually transmits.
fn random_profile(random: &mut XorShift64, media_datagram_bytes: u16) -> NetworkProfile {
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
    // Occasionally shape the link and bound the sender-side socket buffer so
    // the walk exercises finite-link serialization and backpressure. Slow
    // links guarantee shaping pressure; tight buffers guarantee backpressure.
    let shaped = random.percent() >= 10;
    network.bandwidth_bytes_per_second = if shaped && random.percent() < 30 {
        u64::try_from(random.next_usize(4_801) + 200).unwrap() // 200..=5_000 bytes/s
    } else if shaped {
        u64::try_from(random.next_usize(1_995_001) + 5_000).unwrap() // 5_000..=2_000_000 bytes/s
    } else {
        0
    };
    network.max_in_flight_packets = random.next_usize(15) + 2;
    // Mix bufferless, tight, and roomy sender-side socket buffers.
    network.send_buffer_bytes = if !shaped || random.percent() < 30 {
        None
    } else if random.percent() < 50 {
        Some(u64::try_from(random.next_usize(usize::from(media_datagram_bytes) * 8) + 1).unwrap())
    } else {
        Some(u64::try_from(random.next_usize(2_000_000) + 1).unwrap())
    };
    network
}

/// Asserts the finite-link and sender-side backpressure invariants that hold
/// for every completed report.
fn assert_network_invariants<const S: usize>(
    config: &MixerControllerScenarioConfig<S>,
    report: &MixerControllerSimulationReport<S, SLOTS>,
) {
    let bandwidth = config.network.bandwidth_bytes_per_second;
    if bandwidth == 0 {
        assert_eq!(
            report.network.bandwidth_shaped_copies, 0,
            "an unshaped link must not serialize any copy"
        );
        assert_eq!(
            report.network.bandwidth_serialized_bytes, 0,
            "an unshaped link must not place bytes on a link"
        );
    } else {
        assert_eq!(
            report.network.bandwidth_shaped_copies, report.network.scheduled_copies,
            "every scheduled copy is serialized on the finite link"
        );
        assert_eq!(
            report.network.bandwidth_serialized_bytes,
            report.network.scheduled_copies * u64::from(config.media_datagram_bytes),
            "serialized bytes equal one datagram per scheduled copy"
        );
    }

    // Without a finite link or without a socket buffer the sender never
    // reports backpressure, by construction of the model.
    if config.network.send_buffer_bytes.is_none() || bandwidth == 0 {
        assert_eq!(
            report.network.send_buffer_full_packets, 0,
            "no finite link or no socket buffer means no backpressure"
        );
        assert_eq!(
            report.network.send_buffer_queued_bytes, 0,
            "no finite link or no socket buffer means no queued bytes"
        );
    } else {
        let capacity = config.network.send_buffer_bytes.expect("checked Some");
        assert!(
            report.network.send_buffer_queued_bytes <= capacity,
            "sender-side queued bytes never exceed the configured socket-buffer capacity"
        );
    }
}

#[test]
fn every_valid_configuration_stays_bounded_and_consumes_every_frame() {
    // Phase 2 admits two to four independent sources; verify both ends of the
    // range over the same seeded profiles.
    let (shaped, backpressure) = check_stream_count::<2>();
    let (shaped_4, backpressure_4) = check_stream_count::<4>();
    assert!(
        shaped + shaped_4 > 0,
        "walk must exercise the finite-link path"
    );
    assert!(
        backpressure + backpressure_4 > 0,
        "walk must exercise sender-side backpressure"
    );
}

/// Drives `STEPS` seeded configurations at a fixed stream count and asserts
/// the scenario invariants on every report, returning how many reports
/// exercised the finite-link and sender-side backpressure paths.
fn check_stream_count<const S: usize>() -> (u32, u32) {
    let mut random = XorShift64(0x5A17_2026);
    let mut shaped_runs = 0_u32;
    let mut backpressure_runs = 0_u32;
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
        let media_datagram_bytes =
            u16::try_from(random.next_in(1, u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap() + 1))
                .unwrap();
        let config = MixerControllerScenarioConfig::<S> {
            frames,
            frame_duration_ms: u64::from(frame_duration_ms),
            target_buffer_ms: u64::from(random.next_in(0, 1_001)),
            media_datagram_bytes,
            network: random_profile(&mut random, media_datagram_bytes),
            gain_scripts,
        };

        let report = simulate_mixer_controller_membership::<S, SLOTS, PCM_BLOCK>(config)
            .expect("the seeded configuration must be valid");
        if config.network.bandwidth_bytes_per_second > 0 {
            shaped_runs += 1;
        }
        if report.network.send_buffer_full_packets > 0 {
            backpressure_runs += 1;
        }
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
        assert_network_invariants(&config, &report);
    }
    (shaped_runs, backpressure_runs)
}
