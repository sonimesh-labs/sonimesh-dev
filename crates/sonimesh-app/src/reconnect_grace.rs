use core::fmt;

/// Lifecycle state of one bounded control-session reconnect grace window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectGraceState {
    /// No reconnect grace window is currently armed.
    Idle,
    /// The control session may reconnect until the armed deadline.
    InGrace,
}

/// Result of one monotonic-clock observation of the reconnect grace window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectGraceObservation {
    /// No grace window is armed.
    Idle,
    /// The window is active.
    InGrace {
        /// Milliseconds remaining before the control session must expire.
        remaining_ms: u64,
    },
    /// The armed deadline passed and the control session must now expire.
    Expired,
}

/// Failure while arming or observing a reconnect grace window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectGraceError {
    /// A grace window is already armed and must finish or reconnect first.
    AlreadyInGrace,
    /// A reconnect window must be non-zero.
    ZeroDuration,
    /// The caller's monotonic clock moved backward; the window is force-expired.
    TimeRegressed,
    /// Arming the window would overflow the monotonic deadline.
    ExpiryOverflow,
}

impl fmt::Display for ReconnectGraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInGrace => formatter.write_str("reconnect grace is already active"),
            Self::ZeroDuration => formatter.write_str("reconnect grace duration must be non-zero"),
            Self::TimeRegressed => formatter.write_str("reconnect grace clock moved backward"),
            Self::ExpiryOverflow => formatter.write_str("reconnect grace deadline overflowed"),
        }
    }
}

impl std::error::Error for ReconnectGraceError {}

/// Clock-driven owner of one bounded reconnect grace window.
///
/// The control session enters `InGrace` after connection loss and must either
/// reconnect with fresh session material before the deadline or expire. A clock
/// regression fails closed by force-expiring the window, so a later forward
/// jump cannot revive an expired session. The controller owns no keys, tokens,
/// or transport state; the control-session coordinator performs the actual
/// expiry when [`Self::observe`] reports [`ReconnectGraceObservation::Expired`].
pub struct ReconnectGraceController {
    state: ReconnectGraceState,
    deadline_ms: u64,
    last_observed_ms: u64,
}

impl ReconnectGraceController {
    /// Creates an idle grace owner with no armed window.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ReconnectGraceState::Idle,
            deadline_ms: 0,
            last_observed_ms: 0,
        }
    }

    /// Returns the current window lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ReconnectGraceState {
        self.state
    }

    /// Arms a fresh reconnect window with a bounded monotonic deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ReconnectGraceError::AlreadyInGrace`] while a window is still
    /// active, [`ReconnectGraceError::ZeroDuration`] for a zero duration, or
    /// [`ReconnectGraceError::ExpiryOverflow`] when `now_ms + duration_ms`
    /// overflows the clock.
    pub fn begin(&mut self, now_ms: u64, duration_ms: u64) -> Result<(), ReconnectGraceError> {
        if self.state == ReconnectGraceState::InGrace {
            return Err(ReconnectGraceError::AlreadyInGrace);
        }
        if duration_ms == 0 {
            return Err(ReconnectGraceError::ZeroDuration);
        }
        let deadline_ms = now_ms
            .checked_add(duration_ms)
            .ok_or(ReconnectGraceError::ExpiryOverflow)?;
        self.state = ReconnectGraceState::InGrace;
        self.deadline_ms = deadline_ms;
        self.last_observed_ms = now_ms;
        Ok(())
    }

    /// Observes the window with the caller's monotonic clock.
    ///
    /// An active window that passed its deadline returns
    /// [`ReconnectGraceObservation::Expired`] and returns to idle, so the
    /// control-session owner must expire its session. A clock regression
    /// force-expires the window and fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`ReconnectGraceError::TimeRegressed`] after force-expiring an
    /// active window when `now_ms` is earlier than a prior observation.
    pub fn observe(
        &mut self,
        now_ms: u64,
    ) -> Result<ReconnectGraceObservation, ReconnectGraceError> {
        if self.state == ReconnectGraceState::Idle {
            return Ok(ReconnectGraceObservation::Idle);
        }
        if now_ms < self.last_observed_ms {
            self.reset();
            return Err(ReconnectGraceError::TimeRegressed);
        }
        self.last_observed_ms = now_ms;
        if now_ms >= self.deadline_ms {
            self.reset();
            return Ok(ReconnectGraceObservation::Expired);
        }
        Ok(ReconnectGraceObservation::InGrace {
            remaining_ms: self.deadline_ms - now_ms,
        })
    }

    /// Ends the window because the session reconnected with fresh material.
    ///
    /// Returns whether a window was active before the reset.
    #[must_use]
    pub fn reconnected(&mut self) -> bool {
        let was_active = self.state == ReconnectGraceState::InGrace;
        self.reset();
        was_active
    }

    /// Forcibly ends the window without a reconnect (expiry or abandonment).
    ///
    /// Returns whether a window was active before the reset.
    #[must_use]
    pub fn cancel(&mut self) -> bool {
        let was_active = self.state == ReconnectGraceState::InGrace;
        self.reset();
        was_active
    }

    fn reset(&mut self) {
        self.state = ReconnectGraceState::Idle;
        self.deadline_ms = 0;
        self.last_observed_ms = 0;
    }
}

impl Default for ReconnectGraceController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ReconnectGraceController, ReconnectGraceError, ReconnectGraceObservation,
        ReconnectGraceState,
    };

    #[test]
    fn idle_observation_is_a_noop() {
        let mut grace = ReconnectGraceController::new();
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
        assert_eq!(grace.observe(1_000), Ok(ReconnectGraceObservation::Idle));
        assert_eq!(grace.observe(u64::MAX), Ok(ReconnectGraceObservation::Idle));
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
    }

    #[test]
    fn begin_arms_a_bounded_deadline_and_reports_remaining() {
        let mut grace = ReconnectGraceController::new();
        assert_eq!(grace.begin(100, 2_000), Ok(()));
        assert_eq!(grace.state(), ReconnectGraceState::InGrace);
        assert_eq!(
            grace.observe(100),
            Ok(ReconnectGraceObservation::InGrace {
                remaining_ms: 2_000
            })
        );
        assert_eq!(
            grace.observe(1_500),
            Ok(ReconnectGraceObservation::InGrace { remaining_ms: 600 })
        );
        assert_eq!(grace.state(), ReconnectGraceState::InGrace);
    }

    #[test]
    fn observe_expires_at_and_after_the_deadline_then_returns_to_idle() {
        let mut grace = ReconnectGraceController::new();
        grace.begin(100, 2_000).unwrap();
        assert_eq!(
            grace.observe(2_099),
            Ok(ReconnectGraceObservation::InGrace { remaining_ms: 1 })
        );
        assert_eq!(grace.observe(2_100), Ok(ReconnectGraceObservation::Expired));
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
        assert_eq!(grace.observe(2_100), Ok(ReconnectGraceObservation::Idle));
        assert_eq!(grace.observe(9_999), Ok(ReconnectGraceObservation::Idle));
    }

    #[test]
    fn clock_regression_force_expires_and_fails_closed() {
        let mut grace = ReconnectGraceController::new();
        grace.begin(1_000, 5_000).unwrap();
        assert_eq!(
            grace.observe(1_200),
            Ok(ReconnectGraceObservation::InGrace {
                remaining_ms: 4_800
            })
        );
        assert_eq!(grace.observe(900), Err(ReconnectGraceError::TimeRegressed));
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
        // A later forward jump cannot revive the force-expired window.
        assert_eq!(grace.observe(2_000), Ok(ReconnectGraceObservation::Idle));
    }

    #[test]
    fn begin_rejects_zero_and_overflowing_durations() {
        let mut grace = ReconnectGraceController::new();
        assert_eq!(grace.begin(100, 0), Err(ReconnectGraceError::ZeroDuration));
        assert_eq!(
            grace.begin(u64::MAX, 1),
            Err(ReconnectGraceError::ExpiryOverflow)
        );
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
    }

    #[test]
    fn begin_rejects_an_already_active_window() {
        let mut grace = ReconnectGraceController::new();
        grace.begin(100, 1_000).unwrap();
        assert_eq!(
            grace.begin(200, 1_000),
            Err(ReconnectGraceError::AlreadyInGrace)
        );
        assert_eq!(grace.state(), ReconnectGraceState::InGrace);
    }

    #[test]
    fn reconnected_and_cancel_end_an_active_window() {
        let mut grace = ReconnectGraceController::new();
        grace.begin(100, 1_000).unwrap();
        assert!(grace.reconnected());
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
        assert!(!grace.reconnected());

        grace.begin(200, 1_000).unwrap();
        assert!(grace.cancel());
        assert_eq!(grace.state(), ReconnectGraceState::Idle);
        assert!(!grace.cancel());
    }

    #[test]
    fn window_can_be_rearmed_after_expiry_or_reconnect() {
        let mut grace = ReconnectGraceController::new();
        grace.begin(100, 10).unwrap();
        assert_eq!(grace.observe(200), Ok(ReconnectGraceObservation::Expired));
        assert_eq!(grace.begin(300, 10), Ok(()));
        assert_eq!(
            grace.observe(300),
            Ok(ReconnectGraceObservation::InGrace { remaining_ms: 10 })
        );
        assert!(grace.reconnected());
        assert_eq!(grace.begin(400, 10), Ok(()));
    }
}
