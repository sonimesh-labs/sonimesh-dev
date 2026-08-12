//! Deterministic property coverage for the adaptive jitter target controller.
//!
//! The controller is a bounded scheduling-domain decision: the target must
//! always stay inside `[min, max]`, must move by at most one frame per
//! observation regardless of input, and must return to the mode base on reset.
//! This test drives seeded random jitter and water-level observations and
//! asserts those invariants over a long run.

use sonimesh_audio::{
    AdaptiveJitterTarget, JitterMode, JitterTargetConfig, JitterTargetStep,
    MAX_JITTER_TARGET_FRAMES,
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

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
}

const STEPS: usize = 20_000;

#[test]
fn target_stays_bounded_and_moves_at_most_one_frame_per_observation() {
    for seed in [0x1234_5678_9abc_def0, 0x0bad_c0de_00ff_00ff] {
        let mut random = XorShift64(seed);
        let capacity = 8 + random.next_usize(24);
        let config = JitterTargetConfig::preset(JitterMode::Balanced, 10, capacity);
        let mut controller = AdaptiveJitterTarget::new(config).unwrap();
        let min_frames = controller.min_frames();
        let max_frames = controller.max_frames();

        for _ in 0..STEPS {
            let jitter_ms = random.next_u32() % 200;
            let buffered = random.next_usize(capacity + 1);
            let before = controller.target_frames();
            let step = controller.observe(jitter_ms, buffered);
            let after = controller.target_frames();

            assert!(
                (min_frames..=max_frames).contains(&after),
                "target {after} escaped [{min_frames}, {max_frames}]"
            );
            assert!(
                after.abs_diff(before) <= 1,
                "target moved {before} -> {after} in one observation"
            );
            assert!(matches!(
                step,
                JitterTargetStep::Held | JitterTargetStep::Raised | JitterTargetStep::Lowered
            ));
        }
    }
}

#[test]
fn hard_bound_never_exceeds_the_fixed_frame_cap() {
    // Any valid configuration, including 1 ms frames and a very large
    // capacity, must keep the derived maximum within the fixed controller cap.
    let config = JitterTargetConfig::preset(JitterMode::Stable, 1, 512);
    let controller = AdaptiveJitterTarget::new(config).unwrap();
    assert!(controller.max_frames() <= MAX_JITTER_TARGET_FRAMES);
    assert!(controller.target_frames() <= controller.max_frames());
}

#[test]
fn reset_restores_the_base_and_clears_history() {
    let mut random = XorShift64(0xdec0_1234_5678_9abc);
    let capacity = 8 + random.next_usize(24);
    let config = JitterTargetConfig::preset(JitterMode::Stable, 20, capacity);
    let mut controller = AdaptiveJitterTarget::new(config).unwrap();
    let base_frames = controller.base_frames();

    for _ in 0..STEPS {
        let _ = controller.observe(random.next_u32() % 300, random.next_usize(capacity + 1));
    }
    controller.reset();
    assert_eq!(controller.target_frames(), base_frames);
    assert_eq!(controller.smoothed_jitter_ms(), 0);
    // One observation after reset cannot move the target immediately.
    assert_eq!(controller.observe(200, capacity), JitterTargetStep::Held);
    assert_eq!(controller.target_frames(), base_frames);
}
