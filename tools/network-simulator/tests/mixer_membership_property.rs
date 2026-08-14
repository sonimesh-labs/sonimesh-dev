//! Deterministic property coverage for the fixed-slot mixer membership
//! simulation over a shared virtual link.
//!
//! ADR 0015's fixed-slot `CommandDrivenMixer` is driven from a multi-stream
//! shared-link run: every stream's playout is decoded to a finite PCM block
//! and pushed into its slot, the callback renders the bus every tick, and a
//! mid-session remove/reinsert exercises membership churn. Every accepted
//! report must consume every frame through playout or PLC, submit exactly one
//! packet per stream per frame, keep both streams active in the fixed-slot
//! table after the reinsert, publish the two starts plus one reinsert and one
//! remove, and render a finite bus. This test drives seeded random network
//! profiles over a long run and asserts those invariants on every report,
//! mirroring the deterministic property pattern used for the other mixer
//! scenarios.

use sonimesh_network_simulator::{
    MixerMembershipScenarioConfig, MixerMembershipSimulationReport, NetworkProfile,
    simulate_mixer_membership,
};

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

    /// Returns a value in `low..high_exclusive` for `low < high_exclusive`.
    fn next_in(&mut self, low: u32, high_exclusive: u32) -> u32 {
        low + self.next_u32() % (high_exclusive - low)
    }
}

/// Number of seeded configurations per property run.
const STEPS: usize = 128;
/// Two source streams share one link and the mixer table.
const STREAMS: usize = 2;
/// Fixed slot count of the mixer membership table.
const SLOTS: usize = 4;
/// Output-format PCM block size used by the mixer callback.
const PCM_BLOCK: usize = 64;

/// Builds a validated network profile within the scenario impairment range.
fn random_profile(random: &mut XorShift64) -> NetworkProfile {
    let mut network = NetworkProfile::stable(u64::from(random.next_u32()));
    network.loss_percent = u8::try_from(random.next_in(0, 16)).unwrap();
    network.duplicate_percent = u8::try_from(random.next_in(0, 11)).unwrap();
    network.reorder_percent = u8::try_from(random.next_in(0, 9)).unwrap();
    network.base_delay_ms = u64::from(random.next_in(1, 61));
    network.jitter_ms = u64::from(random.next_in(0, 121));
    network.reorder_delay_ms = u64::from(random.next_in(0, 41));
    if random.next_u32() % 3 == 0 {
        let interval = random.next_in(20, 61);
        network.burst_interval_packets = interval;
        network.burst_length_packets =
            u16::try_from(random.next_in(1, (interval - 1).min(6) + 1)).unwrap();
    }
    network
}

/// Builds one valid configuration from the seeded random stream.
fn random_config(random: &mut XorShift64) -> MixerMembershipScenarioConfig<STREAMS> {
    MixerMembershipScenarioConfig {
        frames: random.next_in(30, 201),
        frame_duration_ms: if random.next_u32() % 2 == 0 { 10 } else { 20 },
        target_buffer_ms: u64::from(random.next_in(10, 301)),
        media_datagram_bytes: 256,
        network: random_profile(random),
    }
}

/// Asserts every mixer-membership invariant on one completed report.
fn assert_invariants(
    config: &MixerMembershipScenarioConfig<STREAMS>,
    report: &MixerMembershipSimulationReport<STREAMS, SLOTS>,
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
        report.mixer.active_streams, STREAMS,
        "both streams must be active after the reinsert"
    );
    assert_eq!(
        report.mixer.published_inserts, 3,
        "two starts plus the mid-session reinsert"
    );
    assert_eq!(report.mixer.published_removes, 1, "the mid-session remove");
    assert!(report.mixer.renders > 0, "the callback must render the bus");
    assert!(report.mixer.output_finite, "the bus must stay finite");
}

#[test]
fn every_configuration_completes_with_bounded_mixer_bus() {
    for seed in [0x5EED_2026_0814_0002, 0xABCD_EF01_2345_6792] {
        let mut random = XorShift64(seed);
        for _ in 0..STEPS {
            let config = random_config(&mut random);
            let report = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config)
                .unwrap_or_else(|error| panic!("configuration {config:?} failed: {error}"));
            assert_invariants(&config, &report);
        }
    }
}

#[test]
fn identical_configs_produce_identical_reports() {
    let mut random = XorShift64(0xDEAD_BEEF_2026_0814);
    for _ in 0..16 {
        let config = random_config(&mut random);
        let first = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config).unwrap();
        let second = simulate_mixer_membership::<STREAMS, SLOTS, PCM_BLOCK>(config).unwrap();
        assert_eq!(first, second);
    }
}
