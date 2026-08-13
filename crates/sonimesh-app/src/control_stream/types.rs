use sonimesh_protocol::{MediaCryptoError, MediaDirection, MediaKeyContext, NegotiatedControl};

/// Opaque identifier for one installed authenticated control transport.
///
/// The transport owner must retain this token with the TLS connection that was
/// authenticated for the current session. A token from a retired connection is
/// rejected even when its delayed envelope is otherwise valid. Values are
/// unique across coordinators for the lifetime of the process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlSessionToken(pub(super) u64);

/// Opaque, non-reusable identifier for a pending control-domain operation.
///
/// The coordinator issues this process-unique token with a start, rotation, or
/// stop action. The platform owner must return the same token when acknowledging
/// completion or failure, so a late or cross-coordinator result cannot affect
/// different pending work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlOperationToken(pub(super) u64);

/// Exact media identity bound to one session, stream, and AEAD key epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaStreamIdentity {
    /// Random identifier for the active media session.
    pub session_id: u64,
    /// Stream identifier within the media session.
    pub stream_id: u32,
    /// Explicit AEAD key generation for this stream.
    pub key_epoch: u16,
}

impl MediaStreamIdentity {
    /// Builds the protocol key context for one fixed media direction.
    ///
    /// This copies only public media identity fields. The authenticated root
    /// secret remains owned by the pairing/control-session layer.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::InvalidContext`] if a caller manually
    /// constructs an identity with a zero session or stream ID.
    pub const fn media_key_context(
        self,
        direction: MediaDirection,
    ) -> Result<MediaKeyContext, MediaCryptoError> {
        MediaKeyContext::new(self.session_id, self.stream_id, self.key_epoch, direction)
    }
}

/// Validated audio parameters carried by a `StartStream` control command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamStartRequest {
    /// Media identity to install after platform/output preparation succeeds.
    pub identity: MediaStreamIdentity,
    /// Sender PCM sample rate to configure for the stream pipeline.
    pub sample_rate_hz: u32,
    /// Sender channel count to configure for the stream pipeline.
    pub channels: u32,
    /// Encoded frame duration selected by control negotiation.
    pub frame_duration_ms: u32,
}

/// Low-rate action prepared from one authenticated control envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlStreamAction {
    /// The authenticated peer hello selected a fixed control capability set.
    Negotiated(NegotiatedControl),
    /// Prepare platform output and stream-local media state for a new stream.
    Start {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Validated stream parameters for platform and media preparation.
        request: StreamStartRequest,
    },
    /// Replace media state without changing the already-running output stream.
    Rotate {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Currently active media identity to retire.
        previous: MediaStreamIdentity,
        /// New epoch and configuration to install.
        next: StreamStartRequest,
    },
    /// Stop the exact active media identity.
    Stop {
        /// Opaque identifier required to acknowledge this operation.
        token: ControlOperationToken,
        /// Exact media identity to stop.
        identity: MediaStreamIdentity,
    },
    /// Command has no state change because it repeats known completed work.
    Noop,
    /// Envelope is valid but is not a stream lifecycle command for this owner.
    Ignored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingStreamChange {
    Start {
        token: ControlOperationToken,
        request: StreamStartRequest,
    },
    Rotate {
        token: ControlOperationToken,
        previous: MediaStreamIdentity,
        next: StreamStartRequest,
    },
    Stop {
        token: ControlOperationToken,
        identity: MediaStreamIdentity,
    },
}

impl PendingStreamChange {
    pub(super) const fn start_request(self) -> Option<StreamStartRequest> {
        match self {
            Self::Start { request, .. } => Some(request),
            Self::Rotate { next, .. } => Some(next),
            Self::Stop { .. } => None,
        }
    }

    pub(super) const fn token(self) -> ControlOperationToken {
        match self {
            Self::Start { token, .. } | Self::Rotate { token, .. } | Self::Stop { token, .. } => {
                token
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{MediaCryptoError, MediaDirection};

    use super::{
        ControlOperationToken, ControlSessionToken, ControlStreamAction, MediaStreamIdentity,
        PendingStreamChange, StreamStartRequest,
    };

    fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id,
            stream_id,
            key_epoch,
        }
    }

    #[test]
    fn media_key_context_round_trips_exact_fields_for_both_directions() {
        let identity = identity(0x0102_0304_0506_0708, 0x0001_0002, 3);
        for direction in [
            MediaDirection::AndroidToDesktop,
            MediaDirection::DesktopToAndroid,
        ] {
            let context = identity
                .media_key_context(direction)
                .expect("non-zero media identity is a valid context");
            assert_eq!(context.session_id, identity.session_id);
            assert_eq!(context.stream_id, identity.stream_id);
            assert_eq!(context.key_epoch, identity.key_epoch);
            assert_eq!(context.direction, direction);
        }
    }

    #[test]
    fn media_key_context_rejects_zero_session_or_stream_identifiers() {
        for direction in [
            MediaDirection::AndroidToDesktop,
            MediaDirection::DesktopToAndroid,
        ] {
            assert!(matches!(
                identity(0, 1, 0).media_key_context(direction),
                Err(MediaCryptoError::InvalidContext)
            ));
            assert!(matches!(
                identity(1, 0, 0).media_key_context(direction),
                Err(MediaCryptoError::InvalidContext)
            ));
            assert!(identity(1, 1, 0).media_key_context(direction).is_ok());
        }
    }

    #[test]
    fn coordinator_values_remain_copy_so_actions_can_be_repeated() {
        // The coordinator returns these by value to a platform owner that must
        // be able to acknowledge the same operation later, so every value that
        // crosses that boundary must stay Copy rather than silently becoming a
        // clone-only type.
        fn assert_copy<T: Copy>() {}
        assert_copy::<ControlSessionToken>();
        assert_copy::<ControlOperationToken>();
        assert_copy::<MediaStreamIdentity>();
        assert_copy::<StreamStartRequest>();
        assert_copy::<ControlStreamAction>();
        assert_copy::<PendingStreamChange>();
    }
}
