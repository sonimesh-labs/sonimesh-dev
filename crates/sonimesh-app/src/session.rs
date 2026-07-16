use core::fmt;

/// High-level desktop session state exposed to low-rate UI consumers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionState(SessionPhase);

/// Read-only lifecycle phase for UI and platform control consumers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionPhase {
    /// No trusted media session is active.
    #[default]
    Idle,
    /// A short-lived first-pairing record is active.
    Pairing,
    /// Trusted control session is ready but not streaming.
    Ready,
    /// Authenticated media is being received.
    Streaming,
    /// Control or network reachability is within a bounded grace period.
    Reconnecting,
    /// Output device must be recreated by the control domain.
    OutputUnavailable,
}

/// Control-domain event that may advance a session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionEvent {
    /// User starts an expiring pairing attempt.
    StartPairing,
    /// Authenticated sender starts one media stream.
    StartStream,
    /// Stop is idempotent from any connected streaming state.
    StopStream,
    /// Control channel or local network is temporarily lost.
    ConnectionLost,
    /// Trusted session reconnects with fresh session material.
    Reconnected,
    /// Active output disappears and must be recreated elsewhere.
    OutputLost,
    /// Output has been recreated outside the realtime callback.
    OutputRestored,
    /// Trust or the bounded reconnect grace period ends.
    SessionExpired,
}

/// Invalid lifecycle transition rejected by the coordinator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionTransitionError {
    /// State before the rejected event.
    pub state: SessionState,
    /// Event that is not valid from `state`.
    pub event: SessionEvent,
}

impl fmt::Display for SessionTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "session event {:?} is invalid from {:?}",
            self.event, self.state
        )
    }
}

impl std::error::Error for SessionTransitionError {}

impl SessionState {
    /// No trusted media session is active.
    pub const IDLE: Self = Self(SessionPhase::Idle);
    /// A short-lived first-pairing record is active.
    pub(crate) const PAIRING: Self = Self(SessionPhase::Pairing);
    /// Trusted control session is ready but not streaming.
    pub(crate) const READY: Self = Self(SessionPhase::Ready);
    /// Authenticated media is being received.
    pub(crate) const STREAMING: Self = Self(SessionPhase::Streaming);
    /// Control or network reachability is within a bounded grace period.
    pub(crate) const RECONNECTING: Self = Self(SessionPhase::Reconnecting);
    /// Output device must be recreated by the control domain.
    pub(crate) const OUTPUT_UNAVAILABLE: Self = Self(SessionPhase::OutputUnavailable);

    /// Returns the current read-only lifecycle phase.
    #[must_use]
    pub const fn phase(self) -> SessionPhase {
        self.0
    }

    /// Applies a validated lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTransitionError`] for events not defined from the current
    /// state. Repeated [`SessionEvent::StopStream`] is intentionally safe.
    pub fn apply(&mut self, event: SessionEvent) -> Result<(), SessionTransitionError> {
        if event == SessionEvent::StopStream {
            if matches!(
                *self,
                Self::STREAMING | Self::RECONNECTING | Self::OUTPUT_UNAVAILABLE
            ) {
                *self = Self::READY;
            }
            return Ok(());
        }

        let next = match (*self, event) {
            (Self::IDLE, SessionEvent::StartPairing) => Self::PAIRING,
            (Self::RECONNECTING, SessionEvent::Reconnected) => Self::READY,
            (Self::READY, SessionEvent::StartStream)
            | (Self::OUTPUT_UNAVAILABLE, SessionEvent::OutputRestored) => Self::STREAMING,
            (Self::STREAMING | Self::READY, SessionEvent::ConnectionLost) => Self::RECONNECTING,
            (Self::STREAMING, SessionEvent::OutputLost) => Self::OUTPUT_UNAVAILABLE,
            (_, SessionEvent::SessionExpired) => Self::IDLE,
            _ => {
                return Err(SessionTransitionError {
                    state: *self,
                    event,
                });
            }
        };
        *self = next;
        Ok(())
    }

    /// Marks a single-use invitation as trusted after its proof was consumed.
    ///
    /// This is crate-private so UI and platform callers cannot independently
    /// create a ready session without the pairing owner's record transition.
    pub(crate) fn establish_trust(&mut self) -> bool {
        if *self != Self::PAIRING {
            return false;
        }
        *self = Self::READY;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_normal_pair_and_stream_path() {
        let mut state = SessionState::IDLE;
        state.apply(SessionEvent::StartPairing).unwrap();
        assert!(state.establish_trust());
        state.apply(SessionEvent::StartStream).unwrap();
        state.apply(SessionEvent::StopStream).unwrap();
        assert_eq!(state, SessionState::READY);
    }

    #[test]
    fn repeated_stop_is_idempotent_when_ready() {
        let mut state = SessionState::READY;
        state.apply(SessionEvent::StopStream).unwrap();
        state.apply(SessionEvent::StopStream).unwrap();
        assert_eq!(state, SessionState::READY);
    }

    #[test]
    fn stop_is_safe_without_an_active_session() {
        let mut state = SessionState::IDLE;
        state.apply(SessionEvent::StopStream).unwrap();
        assert_eq!(state, SessionState::IDLE);
    }

    #[test]
    fn output_recreation_resumes_streaming() {
        let mut state = SessionState::STREAMING;
        state.apply(SessionEvent::OutputLost).unwrap();
        assert_eq!(state, SessionState::OUTPUT_UNAVAILABLE);
        state.apply(SessionEvent::OutputRestored).unwrap();
        assert_eq!(state, SessionState::STREAMING);
    }

    #[test]
    fn rejects_stream_before_trust() {
        let mut state = SessionState::IDLE;
        let error = state.apply(SessionEvent::StartStream).unwrap_err();
        assert_eq!(error.state, SessionState::IDLE);
        assert_eq!(state, SessionState::IDLE);
    }
}
