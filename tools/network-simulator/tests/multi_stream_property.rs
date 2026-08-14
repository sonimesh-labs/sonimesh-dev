//! Deterministic property coverage for the multi-stream shared-link scenario.
//!
//! Two to four source streams traverse one shared virtual link and profile, so
//! correlated loss and bandwidth serialization are observed against independent
//! per-stream jitter buffers and playout deadlines. Every accepted report must
//! be deterministic, must submit exactly one packet per stream per frame over
//! the shared link, and must account for every playout decision: each stream
//! plays out or conceals exactly its configured frame count. This test drives
//! seeded random profiles, frame durations, and initial playout targets across
//! the full Phase 2 stream range (two, three, and four independent sources) and
//! asserts those invariants on every accepted report, mirroring the
//! deterministic property pattern used for the sibling scenarios.

use sonimesh_network_simulator::{
    MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS, MultiStreamScenarioConfig,
    MultiStreamSimulationReport, NetworkProfile, simulate_multi_stream,
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

/// Number of seeded configurations per property run.
const STEPS: usize = 256;
/// Sender frames per stream stay small so every property step is bounded.
const MAX_FRAMES: u32 = 200;

/// Builds a validated network profile within the scenario impairment range.
///
/// The ranges mirror the mixer scenario property tests and fuzz targets so the
/// seeded run covers the same shared-link configuration space.
fn random_profile(random: &mut XorShift64) -> NetworkProfile {
    let mut network = NetworkProfile::stable(u64::from(random.next_u32()));
    network.base_delay_ms = u64::from(random.next_in(0, 101));
    network.jitter_ms = u64::from(random.next_in(0, 41));
    network.reorder_delay_ms = u64::from(random.next_in(0, 101));
    network.loss_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 101)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 101)).unwrap();
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

/// Builds a validated multi-stream configuration whose initial playout target
/// covers the worst-case first-arrival delay, so an intact first packet is
/// always in the jitter buffer by the first playout deadline.
fn random_config(random: &mut XorShift64) -> MultiStreamScenarioConfig {
    let frame_duration_ms = if random.next_usize(2) == 0 {
        10_u64
    } else {
        20_u64
    };
    let network = random_profile(random);
    let worst_case_first_arrival_ms =
        network.base_delay_ms + network.jitter_ms + network.reorder_delay_ms;
    let target_buffer_ms = worst_case_first_arrival_ms
        + u64::from(random.next_in(
            0,
            1_001 - u32::try_from(worst_case_first_arrival_ms).unwrap(),
        ));
    MultiStreamScenarioConfig {
        frames: random.next_in(1, MAX_FRAMES + 1),
        frame_duration_ms,
        target_buffer_ms,
        media_datagram_bytes: u16::try_from(
            random.next_in(1, u32::try_from(MAX_MEDIA_DATAGRAM_LEN).unwrap() + 1),
        )
        .unwrap(),
        network,
    }
}

/// Asserts the shared-link invariants that hold for every accepted report.
fn assert_scenario_invariants<const STREAMS: usize>(
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

/// Drives `STEPS` seeded configurations at a fixed stream count and asserts the
/// scenario invariants on every accepted report.
fn check_stream_count<const S: usize>() {
    let mut random = XorShift64(0x5A17_2026);
    for _ in 0..STEPS {
        let config = random_config(&mut random);
        // Playout is pre-armed at the negotiated start sequence, so a lost or
        // late first packet conceals via PLC rather than reporting NotReady;
        // accepted reports must satisfy the invariants.
        if let Ok(first) = simulate_multi_stream::<S>(config) {
            let second = simulate_multi_stream::<S>(config)
                .expect("an accepted configuration must stay accepted");
            assert_eq!(
                first, second,
                "the shared-link simulation must be deterministic"
            );
            assert_scenario_invariants(&first, &config);
        }
    }
}

#[test]
fn every_accepted_configuration_stays_bounded_deterministic_and_accounted() {
    // Phase 2 admits two to four independent sources; verify every stream count
    // in the range over the same seeded profiles.
    for streams in MIN_MULTI_STREAM_STREAMS..=MAX_MULTI_STREAM_STREAMS {
        match streams {
            2 => check_stream_count::<2>(),
            3 => check_stream_count::<3>(),
            _ => check_stream_count::<4>(),
        }
    }
}
