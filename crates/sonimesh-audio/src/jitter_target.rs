//! Bounded adaptive jitter target control (ADR 0016).
//!
//! The [`JitterBuffer`](crate::JitterBuffer) is a container; scheduling time
//! and adaptive target selection live above it. This controller converts the
//! architecture's per-mode millisecond range
//! ([section 11](../../docs/architecture.md)) into bounded integer frame
//! targets and moves the current target slowly inside `[min, max]`. It is a
//! scheduling-domain primitive: it must never be called from the output
//! callback, and it never consumes PCM queue occupancy (that signal belongs to
//! the drift controller, ADR 0007).

use core::fmt;

/// Hard upper bound on a derived target maximum, in encoded frames.
pub const MAX_JITTER_TARGET_FRAMES: usize = 256;
/// Smallest accepted encoded frame duration in milliseconds.
pub const MIN_JITTER_FRAME_DURATION_MS: u32 = 1;
/// Largest accepted encoded frame duration in milliseconds.
pub const MAX_JITTER_FRAME_DURATION_MS: u32 = 100;

/// Slow-moving jitter estimate divisor: one quarter of the residual per step.
///
/// The debounce counters, not this divisor, provide the slow-change guarantee:
/// a single spike never raises the target, while a sustained change converges
/// within a few observations.
const EMA_DIVISOR: u32 = 4;
/// Observations of sustained high jitter before the target grows one frame.
const RAISE_DEBOUNCE: u8 = 4;
/// Observations of sustained low jitter with surplus before the target shrinks.
const LOWER_DEBOUNCE: u8 = 16;
/// Buffered-frame surplus required above the target before shrinking.
const LOWER_SURPLUS_FRAMES: usize = 1;

/// User-selectable jitter target mode (architecture section 11 presets).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JitterMode {
    /// Stable WLAN, interactive use: 20-40 ms.
    LowLatency,
    /// Default: 40-70 ms.
    #[default]
    Balanced,
    /// High jitter or congested WLAN: 80-120 ms.
    Stable,
}

impl JitterMode {
    /// Returns the `(min, base, max)` target range in milliseconds.
    const fn presets_ms(self) -> (u32, u32, u32) {
        match self {
            Self::LowLatency => (20, 30, 40),
            Self::Balanced => (40, 55, 70),
            Self::Stable => (80, 100, 120),
        }
    }
}

/// Bounded adaptive jitter target configuration for one stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitterTargetConfig {
    /// User-selected target range preset label.
    pub mode: JitterMode,
    /// Minimum target water level in milliseconds.
    pub min_ms: u32,
    /// Mode base target in milliseconds.
    pub base_ms: u32,
    /// Maximum target water level in milliseconds.
    pub max_ms: u32,
    /// Encoded frame duration in milliseconds used for ms-to-frame conversion.
    pub frame_duration_ms: u32,
    /// Fixed encoded-frame capacity of the stream's jitter buffer.
    pub buffer_capacity_frames: usize,
}

impl JitterTargetConfig {
    /// Builds the preset configuration for one mode with the given capacity.
    #[must_use]
    pub const fn preset(
        mode: JitterMode,
        frame_duration_ms: u32,
        buffer_capacity_frames: usize,
    ) -> Self {
        let (min_ms, base_ms, max_ms) = mode.presets_ms();
        Self {
            mode,
            min_ms,
            base_ms,
            max_ms,
            frame_duration_ms,
            buffer_capacity_frames,
        }
    }

    /// Validates the target range, frame duration, and capacity bounds.
    ///
    /// # Errors
    ///
    /// Returns [`JitterTargetConfigError`] when the range is not ordered, the
    /// frame duration is out of range, the capacity is zero, the derived
    /// minimum is below one frame, or the derived maximum exceeds the capacity
    /// or the hard controller bound.
    pub fn validate(self) -> Result<(), JitterTargetConfigError> {
        let frame_duration_ms = self.frame_duration_ms;
        if !(MIN_JITTER_FRAME_DURATION_MS..=MAX_JITTER_FRAME_DURATION_MS)
            .contains(&frame_duration_ms)
        {
            return Err(JitterTargetConfigError::FrameDurationOutOfRange);
        }
        if self.buffer_capacity_frames == 0 {
            return Err(JitterTargetConfigError::ZeroCapacity);
        }
        if self.min_ms > self.base_ms || self.base_ms > self.max_ms {
            return Err(JitterTargetConfigError::InvalidModeRange);
        }
        let min_frames = self.min_ms.div_ceil(frame_duration_ms);
        let base_frames = round_div(self.base_ms, frame_duration_ms);
        let max_frames = self.max_ms.div_ceil(frame_duration_ms);
        if min_frames == 0 {
            return Err(JitterTargetConfigError::MinBelowOneFrame);
        }
        if max_frames as usize > MAX_JITTER_TARGET_FRAMES {
            return Err(JitterTargetConfigError::TargetExceedsHardBound);
        }
        if max_frames as usize > self.buffer_capacity_frames {
            return Err(JitterTargetConfigError::TargetExceedsCapacity);
        }
        if base_frames < min_frames || base_frames > max_frames {
            return Err(JitterTargetConfigError::InvalidModeRange);
        }
        Ok(())
    }
}

/// Invalid bounded adaptive jitter target configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitterTargetConfigError {
    /// Frame duration is outside the bounded millisecond range.
    FrameDurationOutOfRange,
    /// The jitter buffer capacity is zero.
    ZeroCapacity,
    /// The derived minimum target is below one encoded frame.
    MinBelowOneFrame,
    /// The derived maximum target exceeds the jitter buffer capacity.
    TargetExceedsCapacity,
    /// The derived maximum target exceeds the controller hard bound.
    TargetExceedsHardBound,
    /// The mode preset does not satisfy `min <= base <= max` after conversion.
    InvalidModeRange,
}

impl fmt::Display for JitterTargetConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid adaptive jitter target configuration: {self:?}"
        )
    }
}

impl std::error::Error for JitterTargetConfigError {}

/// One bounded adaptive target decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitterTargetStep {
    /// The target grew one frame toward the mode maximum.
    Raised,
    /// The target shrank one frame toward the mode minimum.
    Lowered,
    /// The target was unchanged this observation.
    Held,
}

/// Non-realtime bounded adaptive target controller for one stream.
pub struct AdaptiveJitterTarget {
    config: JitterTargetConfig,
    min_frames: usize,
    base_frames: usize,
    max_frames: usize,
    target_frames: usize,
    smoothed_jitter_ms: u32,
    raise_pressure: u8,
    lower_pressure: u8,
    raised_steps: u32,
    lowered_steps: u32,
}

impl AdaptiveJitterTarget {
    /// Creates a controller at the mode base target with no history.
    ///
    /// # Errors
    ///
    /// Returns [`JitterTargetConfigError`] when the configuration is unbounded
    /// or cannot fit the mode maximum inside the buffer capacity.
    pub fn new(config: JitterTargetConfig) -> Result<Self, JitterTargetConfigError> {
        config.validate()?;
        let frame_duration_ms = config.frame_duration_ms;
        let min_frames = config.min_ms.div_ceil(frame_duration_ms) as usize;
        let base_frames = round_div(config.base_ms, frame_duration_ms) as usize;
        let max_frames = config.max_ms.div_ceil(frame_duration_ms) as usize;
        Ok(Self {
            config,
            min_frames,
            base_frames,
            max_frames,
            target_frames: base_frames,
            smoothed_jitter_ms: 0,
            raise_pressure: 0,
            lower_pressure: 0,
            raised_steps: 0,
            lowered_steps: 0,
        })
    }

    /// Returns the user-selected mode.
    #[must_use]
    pub const fn mode(&self) -> JitterMode {
        self.config.mode
    }

    /// Returns the configured encoded frame duration in milliseconds.
    #[must_use]
    pub const fn frame_duration_ms(&self) -> u32 {
        self.config.frame_duration_ms
    }

    /// Returns the configured jitter buffer capacity in frames.
    #[must_use]
    pub const fn buffer_capacity_frames(&self) -> usize {
        self.config.buffer_capacity_frames
    }

    /// Returns the minimum target water level in frames.
    #[must_use]
    pub const fn min_frames(&self) -> usize {
        self.min_frames
    }

    /// Returns the mode base target in frames.
    #[must_use]
    pub const fn base_frames(&self) -> usize {
        self.base_frames
    }

    /// Returns the maximum target water level in frames.
    #[must_use]
    pub const fn max_frames(&self) -> usize {
        self.max_frames
    }

    /// Returns the current target water level in frames.
    #[must_use]
    pub const fn target_frames(&self) -> usize {
        self.target_frames
    }

    /// Returns the minimum target water level in milliseconds.
    #[must_use]
    pub fn min_ms(&self) -> u32 {
        frames_to_ms(self.min_frames, self.config.frame_duration_ms)
    }

    /// Returns the mode base target in milliseconds.
    #[must_use]
    pub fn base_ms(&self) -> u32 {
        frames_to_ms(self.base_frames, self.config.frame_duration_ms)
    }

    /// Returns the maximum target water level in milliseconds.
    #[must_use]
    pub fn max_ms(&self) -> u32 {
        frames_to_ms(self.max_frames, self.config.frame_duration_ms)
    }

    /// Returns the current target water level in milliseconds.
    #[must_use]
    pub fn target_ms(&self) -> u32 {
        frames_to_ms(self.target_frames, self.config.frame_duration_ms)
    }

    /// Returns the slow packet-arrival-jitter estimate in milliseconds.
    #[must_use]
    pub const fn smoothed_jitter_ms(&self) -> u32 {
        self.smoothed_jitter_ms
    }

    /// Returns the target-raising steps taken in the active epoch.
    #[must_use]
    pub const fn raised_steps(&self) -> u32 {
        self.raised_steps
    }

    /// Returns the target-lowering steps taken in the active epoch.
    #[must_use]
    pub const fn lowered_steps(&self) -> u32 {
        self.lowered_steps
    }

    /// Clears stream-local control history after an external reset.
    ///
    /// The jitter estimate, both debounce counters, the step counters, and the
    /// target return to the mode base.
    pub fn reset(&mut self) {
        self.target_frames = self.base_frames;
        self.smoothed_jitter_ms = 0;
        self.raise_pressure = 0;
        self.lower_pressure = 0;
        self.raised_steps = 0;
        self.lowered_steps = 0;
    }

    /// Converts one packet-arrival-jitter and water-level observation into a
    /// bounded target step.
    ///
    /// The target moves at most one frame per call and only after sustained
    /// conditions: high jitter raises it toward the mode maximum, while low
    /// jitter with buffered surplus lowers it toward the mode minimum. The
    /// dead zone between `target / 2` and `target` prevents oscillation.
    #[must_use]
    pub fn observe(&mut self, arrival_jitter_ms: u32, buffered_frames: usize) -> JitterTargetStep {
        self.update_smoothed_jitter(arrival_jitter_ms);
        let target_ms = self.target_ms();
        let smoothed_jitter_ms = self.smoothed_jitter_ms;

        if smoothed_jitter_ms > target_ms {
            self.lower_pressure = 0;
            self.raise_pressure = self.raise_pressure.saturating_add(1);
            if self.raise_pressure >= RAISE_DEBOUNCE && self.target_frames < self.max_frames {
                self.raise_pressure = 0;
                self.target_frames += 1;
                self.raised_steps = self.raised_steps.saturating_add(1);
                return JitterTargetStep::Raised;
            }
            return JitterTargetStep::Held;
        }
        if smoothed_jitter_ms < target_ms / 2
            && buffered_frames >= self.target_frames.saturating_add(LOWER_SURPLUS_FRAMES)
        {
            self.raise_pressure = 0;
            self.lower_pressure = self.lower_pressure.saturating_add(1);
            if self.lower_pressure >= LOWER_DEBOUNCE && self.target_frames > self.min_frames {
                self.lower_pressure = 0;
                self.target_frames -= 1;
                self.lowered_steps = self.lowered_steps.saturating_add(1);
                return JitterTargetStep::Lowered;
            }
            return JitterTargetStep::Held;
        }
        self.raise_pressure = 0;
        self.lower_pressure = 0;
        JitterTargetStep::Held
    }

    fn update_smoothed_jitter(&mut self, arrival_jitter_ms: u32) {
        let delta = self.smoothed_jitter_ms.abs_diff(arrival_jitter_ms);
        let step = delta / EMA_DIVISOR;
        if arrival_jitter_ms > self.smoothed_jitter_ms {
            self.smoothed_jitter_ms += step;
        } else {
            self.smoothed_jitter_ms -= step;
        }
    }
}

/// Rounding division for bounded positive integers.
const fn round_div(numerator: u32, divisor: u32) -> u32 {
    (numerator + divisor / 2) / divisor
}

/// Converts a bounded frame count to milliseconds without overflow.
fn frames_to_ms(frames: usize, frame_duration_ms: u32) -> u32 {
    u32::try_from(frames)
        .unwrap_or(u32::MAX)
        .saturating_mul(frame_duration_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Balanced preset at 10 ms frames with an 8-frame jitter buffer.
    const CONFIG: JitterTargetConfig = JitterTargetConfig::preset(JitterMode::Balanced, 10, 8);

    #[test]
    fn balanced_preset_matches_the_architecture_range() {
        let controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        assert_eq!(controller.mode(), JitterMode::Balanced);
        assert_eq!(controller.min_ms(), 40);
        assert_eq!(controller.base_ms(), 60);
        assert_eq!(controller.max_ms(), 70);
        assert_eq!(controller.min_frames(), 4);
        assert_eq!(controller.base_frames(), 6);
        assert_eq!(controller.max_frames(), 7);
        assert_eq!(controller.target_frames(), 6);
        assert_eq!(controller.frame_duration_ms(), 10);
    }

    #[test]
    fn low_latency_and_stable_presets_convert_to_bounded_frames() {
        let low =
            AdaptiveJitterTarget::new(JitterTargetConfig::preset(JitterMode::LowLatency, 10, 8))
                .unwrap();
        assert_eq!(
            (low.min_frames(), low.base_frames(), low.max_frames()),
            (2, 3, 4)
        );

        let stable =
            AdaptiveJitterTarget::new(JitterTargetConfig::preset(JitterMode::Stable, 10, 16))
                .unwrap();
        assert_eq!(
            (
                stable.min_frames(),
                stable.base_frames(),
                stable.max_frames()
            ),
            (8, 10, 12)
        );
    }

    #[test]
    fn rejects_frame_duration_out_of_range() {
        for frame_duration_ms in [0, MAX_JITTER_FRAME_DURATION_MS + 1] {
            let config = JitterTargetConfig {
                frame_duration_ms,
                ..CONFIG
            };
            assert!(matches!(
                AdaptiveJitterTarget::new(config),
                Err(JitterTargetConfigError::FrameDurationOutOfRange)
            ));
        }
    }

    #[test]
    fn rejects_zero_capacity() {
        let config = JitterTargetConfig {
            buffer_capacity_frames: 0,
            ..CONFIG
        };
        assert!(matches!(
            AdaptiveJitterTarget::new(config),
            Err(JitterTargetConfigError::ZeroCapacity)
        ));
    }

    #[test]
    fn rejects_an_unordered_target_range() {
        let config = JitterTargetConfig {
            base_ms: CONFIG.min_ms - 1,
            ..CONFIG
        };
        assert!(matches!(
            AdaptiveJitterTarget::new(config),
            Err(JitterTargetConfigError::InvalidModeRange)
        ));
    }

    #[test]
    fn rejects_a_minimum_below_one_frame() {
        // 1 ms minimum with a 100 ms frame duration rounds down to zero frames.
        let config = JitterTargetConfig {
            mode: JitterMode::LowLatency,
            min_ms: 0,
            base_ms: 0,
            max_ms: 0,
            frame_duration_ms: 10,
            buffer_capacity_frames: 2,
        };
        assert!(matches!(
            AdaptiveJitterTarget::new(config),
            Err(JitterTargetConfigError::MinBelowOneFrame)
        ));
    }

    #[test]
    fn rejects_a_maximum_beyond_the_buffer_capacity() {
        let config = JitterTargetConfig {
            buffer_capacity_frames: 6,
            ..CONFIG
        };
        assert!(matches!(
            AdaptiveJitterTarget::new(config),
            Err(JitterTargetConfigError::TargetExceedsCapacity)
        ));
    }

    #[test]
    fn rejects_a_maximum_beyond_the_hard_frame_bound() {
        let config = JitterTargetConfig {
            mode: JitterMode::Stable,
            min_ms: 10_000,
            base_ms: 10_000,
            max_ms: 10_000,
            frame_duration_ms: 1,
            buffer_capacity_frames: 300,
        };
        assert!(matches!(
            AdaptiveJitterTarget::new(config),
            Err(JitterTargetConfigError::TargetExceedsHardBound)
        ));
    }

    #[test]
    fn target_starts_at_base_and_exposes_water_levels() {
        let controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        assert_eq!(controller.target_frames(), controller.base_frames());
        assert_eq!(controller.target_ms(), controller.base_ms());
        assert_eq!(controller.smoothed_jitter_ms(), 0);
        assert_eq!(controller.buffer_capacity_frames(), 8);
    }

    #[test]
    fn sustained_high_jitter_raises_the_target_slowly_to_the_maximum() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        // Constant 80 ms jitter: the smoothed estimate converges and the
        // debounce window raises the 60 ms base target one frame at a time up
        // to the 70 ms maximum.
        let mut raised = 0;
        for _ in 0..32 {
            if controller.observe(80, 8) == JitterTargetStep::Raised {
                raised += 1;
            }
        }
        assert!(raised >= 1, "sustained high jitter must raise the target");
        assert_eq!(controller.target_frames(), controller.max_frames());
        // Once at the maximum, continued pressure holds.
        for _ in 0..16 {
            assert_eq!(controller.observe(80, 8), JitterTargetStep::Held);
        }
        assert_eq!(controller.target_frames(), controller.max_frames());
    }

    #[test]
    fn stable_jitter_with_surplus_lowers_the_target_to_the_minimum() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        // Reaching the maximum first, then sustained zero jitter with a full
        // buffer must lower the target all the way to the minimum.
        for _ in 0..12 {
            let _ = controller.observe(80, 8);
        }
        assert_eq!(controller.target_frames(), controller.max_frames());
        let mut lowered = 0;
        for _ in 0..128 {
            if controller.observe(0, 8) == JitterTargetStep::Lowered {
                lowered += 1;
            }
        }
        assert!(lowered >= 1, "sustained low jitter with surplus must lower");
        assert_eq!(controller.target_frames(), controller.min_frames());
    }

    #[test]
    fn holding_requires_surplus_before_lowering() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        // Zero jitter but an empty buffer: the shrink condition is gated by the
        // surplus requirement, so the target never leaves the base.
        for _ in 0..64 {
            assert_eq!(controller.observe(0, 0), JitterTargetStep::Held);
        }
        assert_eq!(controller.target_frames(), controller.base_frames());
    }

    #[test]
    fn alternating_observations_do_not_oscillate() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        // 80 ms and 20 ms jitter alternate around the 60 ms base target. The
        // smoothed estimate converges to 50 ms, inside the dead zone between
        // `target / 2` and `target`, so neither branch ever sustains pressure.
        for _ in 0..128 {
            let _ = controller.observe(80, 8);
            let _ = controller.observe(20, 8);
        }
        assert_eq!(controller.target_frames(), controller.base_frames());
    }

    #[test]
    fn each_step_moves_at_most_one_frame() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        for _ in 0..256 {
            let before = controller.target_frames();
            let step = controller.observe(200, 8);
            let after = controller.target_frames();
            assert!(
                after.abs_diff(before) <= 1,
                "a single observation must move the target by at most one frame"
            );
            assert!(matches!(
                step,
                JitterTargetStep::Held | JitterTargetStep::Raised | JitterTargetStep::Lowered
            ));
        }
    }

    #[test]
    fn reset_returns_to_base_and_clears_history() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        for _ in 0..12 {
            let _ = controller.observe(80, 8);
        }
        assert_eq!(controller.target_frames(), controller.max_frames());
        assert!(controller.smoothed_jitter_ms() > 0);

        controller.reset();
        assert_eq!(controller.target_frames(), controller.base_frames());
        assert_eq!(controller.smoothed_jitter_ms(), 0);
        // Pressure counters are cleared: a single observation after reset must
        // not raise the target immediately.
        assert_eq!(controller.observe(80, 8), JitterTargetStep::Held);
        assert_eq!(controller.target_frames(), controller.base_frames());
    }

    #[test]
    fn smoothed_jitter_moves_toward_the_observation() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        assert_eq!(controller.observe(80, 8), JitterTargetStep::Held);
        // 80 / 4 = 20 ms after the first observation.
        assert_eq!(controller.smoothed_jitter_ms(), 20);
        for _ in 0..7 {
            let _ = controller.observe(80, 8);
        }
        // After the debounce window the estimate has approached 80 ms.
        assert!(controller.smoothed_jitter_ms() >= 70);
    }

    #[test]
    fn step_counters_match_observed_raised_and_lowered_steps() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        assert_eq!(
            (controller.raised_steps(), controller.lowered_steps()),
            (0, 0)
        );

        let mut raised = 0;
        for _ in 0..32 {
            if controller.observe(80, 8) == JitterTargetStep::Raised {
                raised += 1;
            }
        }
        assert!(raised >= 1, "sustained high jitter must raise the target");
        assert_eq!(controller.raised_steps(), raised);
        assert_eq!(controller.lowered_steps(), 0);

        let mut lowered = 0;
        for _ in 0..128 {
            if controller.observe(0, 8) == JitterTargetStep::Lowered {
                lowered += 1;
            }
        }
        assert!(lowered >= 1, "sustained surplus must lower the target");
        assert_eq!(controller.lowered_steps(), lowered);
        assert_eq!(controller.raised_steps(), raised);

        // Held observations never touch either step counter: a steady 30 ms
        // observation sits inside the dead zone between `target / 2` and
        // `target`, so neither branch accumulates pressure.
        let raised_before = controller.raised_steps();
        let lowered_before = controller.lowered_steps();
        for _ in 0..16 {
            assert_eq!(controller.observe(30, 8), JitterTargetStep::Held);
        }
        assert_eq!(controller.raised_steps(), raised_before);
        assert_eq!(controller.lowered_steps(), lowered_before);
    }

    #[test]
    fn reset_clears_the_step_counters() {
        let mut controller = AdaptiveJitterTarget::new(CONFIG).unwrap();
        for _ in 0..12 {
            let _ = controller.observe(80, 8);
        }
        assert!(controller.raised_steps() > 0);

        controller.reset();
        assert_eq!(
            (controller.raised_steps(), controller.lowered_steps()),
            (0, 0)
        );
        assert_eq!(controller.smoothed_jitter_ms(), 0);
        assert_eq!(controller.target_frames(), controller.base_frames());
    }
}
