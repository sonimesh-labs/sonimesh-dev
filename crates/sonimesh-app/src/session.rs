use core::fmt;

/// High-level desktop session state exposed to low-rate UI consumers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SessionState {
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
    /// Pairing or trusted reconnection succeeds.
    TrustEstablished,
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
    /// Applies a validated lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTransitionError`] for events not defined from the current
    /// state. Repeated [`SessionEvent::StopStream`] is intentionally safe.
    pub fn apply(&mut self, event: SessionEvent) -> Result<(), SessionTransitionError> {
        if event == SessionEvent::StopStream {
            if matches!(
                self,
                Self::Streaming | Self::Reconnecting | Self::OutputUnavailable
            ) {
                *self = Self::Ready;
            }
            return Ok(());
        }

        let next = match (*self, event) {
            (Self::Idle, SessionEvent::StartPairing) => Self::Pairing,
            (Self::Pairing, SessionEvent::TrustEstablished)
            | (Self::Reconnecting, SessionEvent::Reconnected) => Self::Ready,
            (Self::Ready, SessionEvent::StartStream)
            | (Self::OutputUnavailable, SessionEvent::OutputRestored) => Self::Streaming,
            (Self::Streaming | Self::Ready, SessionEvent::ConnectionLost) => Self::Reconnecting,
            (Self::Streaming, SessionEvent::OutputLost) => Self::OutputUnavailable,
            (_, SessionEvent::SessionExpired) => Self::Idle,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_normal_pair_and_stream_path() {
        let mut state = SessionState::Idle;
        for event in [
            SessionEvent::StartPairing,
            SessionEvent::TrustEstablished,
            SessionEvent::StartStream,
            SessionEvent::StopStream,
        ] {
            state.apply(event).unwrap();
        }
        assert_eq!(state, SessionState::Ready);
    }

    #[test]
    fn repeated_stop_is_idempotent_when_ready() {
        let mut state = SessionState::Ready;
        state.apply(SessionEvent::StopStream).unwrap();
        state.apply(SessionEvent::StopStream).unwrap();
        assert_eq!(state, SessionState::Ready);
    }

    #[test]
    fn stop_is_safe_without_an_active_session() {
        let mut state = SessionState::Idle;
        state.apply(SessionEvent::StopStream).unwrap();
        assert_eq!(state, SessionState::Idle);
    }

    #[test]
    fn output_recreation_resumes_streaming() {
        let mut state = SessionState::Streaming;
        state.apply(SessionEvent::OutputLost).unwrap();
        assert_eq!(state, SessionState::OutputUnavailable);
        state.apply(SessionEvent::OutputRestored).unwrap();
        assert_eq!(state, SessionState::Streaming);
    }

    #[test]
    fn rejects_stream_before_trust() {
        let mut state = SessionState::Idle;
        let error = state.apply(SessionEvent::StartStream).unwrap_err();
        assert_eq!(error.state, SessionState::Idle);
        assert_eq!(state, SessionState::Idle);
    }
}
