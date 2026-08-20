//! Receiver-generated control-plane replies.
//!
//! The authenticated sender issues commands; the receiver answers with
//! [`StreamStatus`] for completed lifecycle transitions and [`ControlError`]
//! for rejected commands. This module is the only producer of those replies:
//! every generated envelope echoes the answered command's request ID and is
//! re-validated against the protocol contract, so the future control-surface
//! command layer can acknowledge commands without reaching into the wire
//! schema or emitting an invalid frame.

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, ControlError, ControlFrameError, ControlNegotiationError, Envelope,
    ErrorCode, StreamState, StreamStatus, envelope, validate_control_envelope,
};

use crate::MediaStreamIdentity;

use super::error::ControlStreamError;

/// One receiver-generated control-plane reply for one command request ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlReply {
    /// A stream lifecycle transition completed for one media identity.
    StreamStatus {
        /// Media identity whose lifecycle transition completed.
        identity: MediaStreamIdentity,
        /// Bounded protocol stream state observed after the transition.
        state: StreamState,
        /// Human-readable detail, bounded by the wire contract.
        detail: String,
    },
    /// A command was rejected with a structured, retryable error.
    Error {
        /// Structured protocol error code for the rejection.
        code: ErrorCode,
        /// Whether the sender may retry the same command unchanged.
        retryable: bool,
        /// Human-readable detail, bounded by the wire contract.
        detail: String,
    },
}

impl ControlReply {
    /// Builds a stream-status reply for one completed lifecycle transition.
    ///
    /// # Errors
    ///
    /// Returns [`ControlFrameError::InvalidStreamStatusIdentity`] for a zero
    /// session or stream identity, [`ControlFrameError::InvalidStreamState`]
    /// for an unspecified state, or [`ControlFrameError::DetailTooLong`] for
    /// an oversized detail.
    pub fn stream_status(
        identity: MediaStreamIdentity,
        state: StreamState,
        detail: &str,
    ) -> Result<Self, ControlFrameError> {
        if identity.session_id == 0 || identity.stream_id == 0 {
            return Err(ControlFrameError::InvalidStreamStatusIdentity);
        }
        if state == StreamState::Unspecified {
            return Err(ControlFrameError::InvalidStreamState(state as i32));
        }
        if detail.len() > sonimesh_protocol::MAX_CONTROL_DETAIL_BYTES {
            return Err(ControlFrameError::DetailTooLong);
        }
        Ok(Self::StreamStatus {
            identity,
            state,
            detail: detail.to_owned(),
        })
    }

    /// Builds a structured error reply for a rejected command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlFrameError::InvalidErrorCode`] for an unspecified code
    /// or [`ControlFrameError::DetailTooLong`] for an oversized detail.
    pub fn error(
        code: ErrorCode,
        retryable: bool,
        detail: &str,
    ) -> Result<Self, ControlFrameError> {
        if code == ErrorCode::Unspecified {
            return Err(ControlFrameError::InvalidErrorCode(code as i32));
        }
        if detail.len() > sonimesh_protocol::MAX_CONTROL_DETAIL_BYTES {
            return Err(ControlFrameError::DetailTooLong);
        }
        Ok(Self::Error {
            code,
            retryable,
            detail: detail.to_owned(),
        })
    }

    /// Echoes the answered command's request ID into a validated envelope.
    ///
    /// The returned envelope always passes the protocol validator, so the
    /// control transport can encode and send it without re-checking bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ControlFrameError`] when the reply cannot be represented as a
    /// protocol-valid envelope.
    pub fn into_envelope(self, request_id: u64) -> Result<Envelope, ControlFrameError> {
        let envelope = match self {
            Self::StreamStatus {
                identity,
                state,
                detail,
            } => Envelope {
                protocol_version: CONTROL_PROTOCOL_VERSION,
                request_id,
                payload: Some(envelope::Payload::StreamStatus(StreamStatus {
                    media_session_id: identity.session_id,
                    stream_id: identity.stream_id,
                    state: state as i32,
                    detail,
                })),
            },
            Self::Error {
                code,
                retryable,
                detail,
            } => Envelope {
                protocol_version: CONTROL_PROTOCOL_VERSION,
                request_id,
                payload: Some(envelope::Payload::Error(ControlError {
                    code: code as i32,
                    retryable,
                    detail,
                })),
            },
        };
        validate_control_envelope(&envelope)?;
        Ok(envelope)
    }

    /// Maps a coordinator rejection into the wire error contract.
    ///
    /// The returned envelope echoes `request_id` so the sender can correlate
    /// the structured error with its rejected command.
    ///
    /// # Errors
    ///
    /// Returns [`ControlFrameError`] when the mapped detail cannot fit the
    /// bounded wire field; the built-in mappings are bounded by construction,
    /// so this is a defensive fail-closed path.
    pub fn for_rejection(
        request_id: u64,
        error: &ControlStreamError,
    ) -> Result<Envelope, ControlFrameError> {
        let (code, retryable) = wire_error_contract(error);
        Self::error(code, retryable, wire_error_detail(error))?.into_envelope(request_id)
    }
}

/// Returns a bounded, stable detail for one wire rejection.
///
/// Nested decoder and platform error strings are intentionally not copied to
/// the wire: they are not part of the control protocol contract and may grow
/// beyond the bounded detail field or expose implementation-specific parser
/// text. The structured code and retry bit carry the machine-readable result;
/// this detail remains short and safe for peer display.
fn wire_error_detail(error: &ControlStreamError) -> &'static str {
    match error {
        ControlStreamError::Protocol(_) => "control command failed protocol validation",
        ControlStreamError::Negotiation(_) => "control hello negotiation failed",
        ControlStreamError::Transition(_) => "control lifecycle transition failed",
        ControlStreamError::ReconnectGrace(_) => "reconnect grace operation failed",
        _ => match wire_error_contract(error).0 {
            ErrorCode::IncompatibleVersion => "control protocol versions are incompatible",
            ErrorCode::UnsupportedCapability => "requested control capability is unsupported",
            ErrorCode::RetryLater => "control operation is temporarily unavailable",
            _ => "control request was rejected",
        },
    }
}

/// Maps one coordinator rejection to the wire error code and retryability.
const fn wire_error_contract(error: &ControlStreamError) -> (ErrorCode, bool) {
    match error {
        ControlStreamError::Negotiation(ControlNegotiationError::NoCommonProtocolVersion) => {
            (ErrorCode::IncompatibleVersion, false)
        }
        ControlStreamError::UnsupportedFrameDuration(_) => {
            (ErrorCode::UnsupportedCapability, false)
        }
        ControlStreamError::PendingChange | ControlStreamError::ActiveStreamLimit => {
            (ErrorCode::RetryLater, true)
        }
        _ => (ErrorCode::InvalidRequest, false),
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{
        ControlFrameError, ControlNegotiationError, Envelope, ErrorCode, StreamState,
        decode_control_frame, encode_control_frame, envelope, validate_control_envelope,
    };

    use crate::{ControlStreamError, MediaStreamIdentity, SessionState};

    use super::ControlReply;

    fn identity() -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id: 7,
            stream_id: 9,
            key_epoch: 0x2345,
        }
    }

    #[test]
    fn status_reply_echoes_the_request_id_and_stays_protocol_valid() {
        let envelope = ControlReply::stream_status(identity(), StreamState::Streaming, "receiving")
            .unwrap()
            .into_envelope(99)
            .unwrap();

        assert_eq!(envelope.protocol_version, 1);
        assert_eq!(envelope.request_id, 99);
        validate_control_envelope(&envelope).unwrap();
    }

    #[test]
    fn error_reply_echoes_the_request_id_and_stays_protocol_valid() {
        let envelope = ControlReply::error(ErrorCode::RetryLater, true, "busy")
            .unwrap()
            .into_envelope(100)
            .unwrap();

        assert_eq!(envelope.request_id, 100);
        validate_control_envelope(&envelope).unwrap();
    }

    #[test]
    fn status_reply_rejects_zero_identities_and_unspecified_state() {
        for session_id in [0, 1] {
            for stream_id in [0, 1] {
                if session_id == 0 || stream_id == 0 {
                    assert!(matches!(
                        ControlReply::stream_status(
                            MediaStreamIdentity {
                                session_id,
                                stream_id,
                                key_epoch: 0,
                            },
                            StreamState::Streaming,
                            "",
                        ),
                        Err(ControlFrameError::InvalidStreamStatusIdentity)
                    ));
                }
            }
        }
        assert!(matches!(
            ControlReply::stream_status(identity(), StreamState::Unspecified, ""),
            Err(ControlFrameError::InvalidStreamState(_))
        ));
    }

    #[test]
    fn error_reply_rejects_unspecified_code() {
        assert!(matches!(
            ControlReply::error(ErrorCode::Unspecified, false, ""),
            Err(ControlFrameError::InvalidErrorCode(_))
        ));
    }

    #[test]
    fn replies_reject_oversized_detail() {
        let detail = "x".repeat(sonimesh_protocol::MAX_CONTROL_DETAIL_BYTES + 1);
        assert!(matches!(
            ControlReply::stream_status(identity(), StreamState::Streaming, &detail),
            Err(ControlFrameError::DetailTooLong)
        ));
        assert!(matches!(
            ControlReply::error(ErrorCode::InvalidRequest, false, &detail),
            Err(ControlFrameError::DetailTooLong)
        ));
    }

    #[test]
    fn rejection_mapping_selects_negotiation_capability_and_retry_contracts() {
        let cases = [
            (
                ControlStreamError::Negotiation(ControlNegotiationError::NoCommonProtocolVersion),
                ErrorCode::IncompatibleVersion as i32,
                false,
            ),
            (
                ControlStreamError::UnsupportedFrameDuration(20),
                ErrorCode::UnsupportedCapability as i32,
                false,
            ),
            (
                ControlStreamError::PendingChange,
                ErrorCode::RetryLater as i32,
                true,
            ),
            (
                ControlStreamError::ActiveStreamLimit,
                ErrorCode::RetryLater as i32,
                true,
            ),
            (
                ControlStreamError::InvalidEpoch,
                ErrorCode::InvalidRequest as i32,
                false,
            ),
            (
                ControlStreamError::StaleEpoch {
                    active: 3,
                    requested: 1,
                },
                ErrorCode::InvalidRequest as i32,
                false,
            ),
            (
                ControlStreamError::StartRequiresReady(SessionState::RECONNECTING),
                ErrorCode::InvalidRequest as i32,
                false,
            ),
        ];
        for (error, code, retryable) in cases {
            let envelope = ControlReply::for_rejection(101, &error).unwrap();
            assert_eq!(envelope.request_id, 101);
            validate_control_envelope(&envelope).unwrap();
            let Some(envelope::Payload::Error(response)) = envelope.payload else {
                panic!("rejection did not produce a ControlError envelope");
            };
            assert_eq!(response.code, code, "wrong wire code for {error:?}");
            assert_eq!(
                response.retryable, retryable,
                "wrong retry flag for {error:?}"
            );
        }
    }

    #[test]
    fn rejection_details_are_stable_and_bounded_for_nested_protocol_errors() {
        let protocol_error = decode_control_frame(&[0, 0, 0, 1, 0x80]).unwrap_err();
        let error = ControlStreamError::Protocol(protocol_error);

        let envelope = ControlReply::for_rejection(204, &error).unwrap();
        let Some(envelope::Payload::Error(response)) = envelope.payload else {
            panic!("protocol rejection did not produce an error response");
        };

        assert_eq!(
            response.detail,
            "control command failed protocol validation"
        );
        assert!(response.detail.len() <= sonimesh_protocol::MAX_CONTROL_DETAIL_BYTES);
    }

    #[test]
    fn reply_envelopes_round_trip_through_the_control_codec() {
        let status: Envelope =
            ControlReply::stream_status(identity(), StreamState::Streaming, "ok")
                .unwrap()
                .into_envelope(202)
                .unwrap();
        let decoded = decode_control_frame(&encode_control_frame(&status).unwrap()).unwrap();
        assert_eq!(decoded, status);

        let error: Envelope = ControlReply::error(ErrorCode::StreamNotFound, false, "gone")
            .unwrap()
            .into_envelope(203)
            .unwrap();
        let decoded = decode_control_frame(&encode_control_frame(&error).unwrap()).unwrap();
        assert_eq!(decoded, error);
    }
}
