use std::fmt;

/// Monotonic deterministic clock used only by simulation code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct VirtualClock {
    now_ms: u64,
}

impl VirtualClock {
    /// Creates a clock at zero milliseconds.
    #[must_use]
    pub const fn new() -> Self {
        Self { now_ms: 0 }
    }

    /// Current virtual monotonic time in milliseconds.
    #[must_use]
    pub const fn now_ms(self) -> u64 {
        self.now_ms
    }

    /// Advances by an exact duration.
    ///
    /// # Errors
    ///
    /// Returns [`ClockError::Overflow`] if virtual time would exceed `u64`.
    pub fn advance_by(&mut self, duration_ms: u64) -> Result<(), ClockError> {
        self.now_ms = self
            .now_ms
            .checked_add(duration_ms)
            .ok_or(ClockError::Overflow)?;
        Ok(())
    }

    /// Advances to an absolute monotonic time.
    ///
    /// # Errors
    ///
    /// Returns [`ClockError::MovedBackwards`] when `target_ms` precedes now.
    pub fn advance_to(&mut self, target_ms: u64) -> Result<(), ClockError> {
        if target_ms < self.now_ms {
            return Err(ClockError::MovedBackwards);
        }
        self.now_ms = target_ms;
        Ok(())
    }
}

/// Invalid virtual-clock operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockError {
    /// Monotonic time cannot decrease.
    MovedBackwards,
    /// Virtual time exceeded its numeric representation.
    Overflow,
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MovedBackwards => formatter.write_str("virtual clock cannot move backwards"),
            Self::Overflow => formatter.write_str("virtual clock overflowed"),
        }
    }
}

impl std::error::Error for ClockError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_backward_time() {
        let mut clock = VirtualClock::new();
        clock.advance_to(10).unwrap();
        assert_eq!(clock.advance_to(9), Err(ClockError::MovedBackwards));
    }
}
