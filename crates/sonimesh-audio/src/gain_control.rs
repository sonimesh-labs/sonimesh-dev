//! Lock-free bounded gain and mute control for one realtime stream.
//!
//! The control domain publishes gain and mute through a cloneable handle; the
//! realtime render path reads the same atomic state without allocation or
//! locking. One handle can be shared across output lifetimes so a user's gain
//! and mute choice survives output-device recreation.

use core::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Largest linear gain the control domain may request.
///
/// The single-stream MVP is attenuation-only; a future release may extend this
/// bound together with headroom and limiter evidence.
pub const MAX_GAIN_LINEAR: f32 = 1.0;

/// Failure while publishing a gain or mute command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GainControlError {
    /// Gain is not finite or is outside the bounded linear range.
    InvalidGain,
}

impl fmt::Display for GainControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGain => {
                formatter.write_str("gain must be finite and within the linear bound")
            }
        }
    }
}

impl std::error::Error for GainControlError {}

/// Snapshot of one stream's gain state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainControlState {
    /// Linear gain target currently published to the realtime source.
    pub gain: f32,
    /// Mute flag currently published to the realtime source.
    pub muted: bool,
}

/// Cross-thread handle for one stream's bounded gain and mute control.
///
/// The realtime render source reads this handle without allocation or locking;
/// the control domain publishes through a clone from a non-realtime owner. One
/// handle can be shared across output lifetimes so a user's gain and mute
/// choice survives output-device recreation.
#[derive(Clone)]
pub struct GainControlHandle {
    gain: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
}

impl GainControlHandle {
    /// Creates an idle handle with unity gain and unmuted state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            muted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Publishes one bounded linear gain for the next realtime callback.
    ///
    /// # Errors
    ///
    /// Returns [`GainControlError::InvalidGain`] without changing state when
    /// `gain` is not finite or is outside `0.0..=MAX_GAIN_LINEAR`.
    pub fn set_gain(&self, gain: f32) -> Result<(), GainControlError> {
        if !gain.is_finite() || !(0.0..=MAX_GAIN_LINEAR).contains(&gain) {
            return Err(GainControlError::InvalidGain);
        }
        self.gain.store(gain.to_bits(), Ordering::Release);
        Ok(())
    }

    /// Publishes one mute command for the next realtime callback.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    /// Returns the last published linear gain.
    #[must_use]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Acquire))
    }

    /// Returns the last published mute flag.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    /// Returns the current published gain and mute state.
    #[must_use]
    pub fn snapshot(&self) -> GainControlState {
        GainControlState {
            gain: self.gain(),
            muted: self.is_muted(),
        }
    }
}

impl Default for GainControlHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{GainControlHandle, GainControlState, MAX_GAIN_LINEAR};

    fn assert_gain(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected gain {expected}, got {actual}"
        );
    }

    #[test]
    fn idle_handle_starts_unity_and_unmuted() {
        let handle = GainControlHandle::new();
        assert_gain(handle.gain(), 1.0);
        assert!(!handle.is_muted());
        assert_eq!(
            handle.snapshot(),
            GainControlState {
                gain: 1.0,
                muted: false
            }
        );
    }

    #[test]
    fn publishes_bounded_gain_and_mute() {
        let handle = GainControlHandle::new();
        assert_eq!(handle.set_gain(0.5), Ok(()));
        assert_gain(handle.gain(), 0.5);
        assert_eq!(handle.set_gain(MAX_GAIN_LINEAR), Ok(()));
        assert_gain(handle.gain(), MAX_GAIN_LINEAR);
        assert_eq!(handle.set_gain(0.0), Ok(()));
        assert_gain(handle.gain(), 0.0);

        handle.set_muted(true);
        assert!(handle.is_muted());
        handle.set_muted(false);
        assert!(!handle.is_muted());
    }

    #[test]
    fn rejects_non_finite_and_out_of_range_gain_without_state_change() {
        let handle = GainControlHandle::new();
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01, 2.0] {
            assert!(handle.set_gain(invalid).is_err(), "accepted {invalid}");
            assert_gain(handle.gain(), 1.0);
        }
        assert_gain(handle.gain(), 1.0);
        assert!(!handle.is_muted());
    }

    #[test]
    fn cloned_handles_share_the_published_state() {
        let owner = GainControlHandle::new();
        let source = owner.clone();
        owner.set_gain(0.25).unwrap();
        owner.set_muted(true);
        assert_gain(source.gain(), 0.25);
        assert!(source.is_muted());
        source.set_gain(0.75).unwrap();
        assert_gain(owner.gain(), 0.75);
    }
}
