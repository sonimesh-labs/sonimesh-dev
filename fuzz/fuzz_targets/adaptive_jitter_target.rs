#![no_main]

//! Fuzz the bounded adaptive jitter controller and its arrival estimator.
//!
//! Arbitrary per-packet arrival ticks and sender sample timestamps feed the
//! relative arrival-jitter estimator, whose deviations then drive the
//! adaptive target controller alongside arbitrary buffered-frame counts.
//! The controller must never panic, must keep the target inside its bounded
//! `[min, max]` water levels, and must never let step counters exceed the
//! observation count; a periodic reset of both the estimator and the
//! controller must retire the target to its mode base so later observations
//! start fresh.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{AdaptiveJitterTarget, ArrivalJitter, JitterMode, JitterTargetConfig};

const SAMPLE_RATE_HZ: u32 = 48_000;
const TICKS_PER_SECOND: u64 = 1_000_000_000;
const FRAME_DURATION_MS: u32 = 20;
const JITTER_BUFFER_CAPACITY: usize = 64;
/// One controller epoch per this many 8-byte inputs.
const RESET_EVERY_CHUNKS: usize = 8;

fuzz_target!(|data: &[u8]| {
    let mut arrival =
        ArrivalJitter::new(SAMPLE_RATE_HZ, TICKS_PER_SECOND).expect("valid estimator rates");
    let mut target = AdaptiveJitterTarget::new(JitterTargetConfig::preset(
        JitterMode::Balanced,
        FRAME_DURATION_MS,
        JITTER_BUFFER_CAPACITY,
    ))
    .expect("valid controller preset");
    let min_frames = target.min_frames();
    let max_frames = target.max_frames();
    let base_frames = target.base_frames();

    let mut observations = 0_u32;
    for (index, chunk) in data.chunks(8).enumerate() {
        // Every epoch retires both estimator and controller so the reset path
        // is exercised and later observations start from a fresh baseline.
        if index % RESET_EVERY_CHUNKS == RESET_EVERY_CHUNKS - 1 {
            arrival.reset();
            target.reset();
            assert_eq!(target.target_frames(), base_frames);
            assert_eq!(target.raised_steps(), 0);
            assert_eq!(target.lowered_steps(), 0);
            observations = 0;
            continue;
        }

        let mut ticks_bytes = [0_u8; 8];
        ticks_bytes[..chunk.len()].copy_from_slice(chunk);
        let arrival_ticks = u64::from_be_bytes(ticks_bytes);
        // Reversing the same bytes gives arbitrary tick/timestamp relations,
        // including backward timestamps that count as reorder jitter.
        let mut timestamp_bytes = ticks_bytes;
        timestamp_bytes.reverse();
        let sample_timestamp = u64::from_be_bytes(timestamp_bytes);

        if let Some(deviation_ms) = arrival.observe(arrival_ticks, sample_timestamp) {
            observations = observations.saturating_add(1);
            let buffered = usize::from(chunk[0]) % (JITTER_BUFFER_CAPACITY + 1);
            let _ = target.observe(deviation_ms, buffered);
        }
        assert!(
            (min_frames..=max_frames).contains(&target.target_frames()),
            "target must stay inside the bounded water levels"
        );
        assert!(
            target.raised_steps() + target.lowered_steps() <= observations,
            "step counters must not exceed observations since the last reset"
        );
    }
});
