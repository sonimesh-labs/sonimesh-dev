use core::fmt;

use sonimesh_protocol::{
    MediaHeader, PacketParseError, ReplayDecision, ReplayWindow, parse_media_packet,
};

/// Session and stream accepted by one ingress instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamIdentity {
    /// Active media session.
    pub session_id: u64,
    /// Stream within that session.
    pub stream_id: u32,
    /// Active key generation.
    pub key_epoch: u16,
}

/// Authenticates and decrypts one bounded packet into caller-owned storage.
///
/// Implementations must authenticate the complete `authenticated_header` as
/// associated data. They must return the plaintext length without allocating.
pub trait Authenticator {
    /// Authentication failure type.
    type Error;

    /// Authenticates `header`/`authenticated_header` and decrypts `ciphertext`
    /// into `plaintext`.
    ///
    /// # Errors
    ///
    /// Returns an implementation-specific error for authentication failure,
    /// invalid ciphertext, or insufficient output capacity.
    fn authenticate(
        &mut self,
        header: &MediaHeader,
        authenticated_header: &[u8],
        ciphertext: &[u8],
        plaintext: &mut [u8],
    ) -> Result<usize, Self::Error>;
}

/// Authenticated media returned by [`MediaIngress`].
#[derive(Debug, Eq, PartialEq)]
pub struct ReceivedFrame<'a> {
    /// Authenticated media metadata.
    pub header: MediaHeader,
    /// Decrypted Opus payload in caller-owned storage.
    pub opus_payload: &'a [u8],
}

/// Failure before an Opus payload may reach the per-stream decoder.
#[derive(Debug, Eq, PartialEq)]
pub enum IngressError<AuthenticationError> {
    /// The unauthenticated envelope is malformed or oversized.
    Parse(PacketParseError),
    /// The packet does not belong to this ingress stream.
    UnknownStream,
    /// AEAD authentication or decryption failed.
    Authentication(AuthenticationError),
    /// Authenticator returned a length beyond caller-owned storage.
    InvalidPlaintextLength,
    /// Authenticated packet is a replay.
    Replay(ReplayDecision),
}

impl<AuthenticationError: fmt::Display> fmt::Display for IngressError<AuthenticationError> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(formatter, "media packet rejected: {error}"),
            Self::UnknownStream => formatter.write_str("media packet belongs to an unknown stream"),
            Self::Authentication(error) => {
                write!(formatter, "media authentication failed: {error}")
            }
            Self::InvalidPlaintextLength => {
                formatter.write_str("authenticator returned an invalid plaintext length")
            }
            Self::Replay(decision) => {
                write!(formatter, "authenticated replay rejected: {decision:?}")
            }
        }
    }
}

impl<AuthenticationError> std::error::Error for IngressError<AuthenticationError>
where
    AuthenticationError: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(error) => Some(error),
            Self::Authentication(error) => Some(error),
            Self::UnknownStream | Self::InvalidPlaintextLength | Self::Replay(_) => None,
        }
    }
}

/// Ordered media ingress for one session, stream, and key epoch.
///
/// The API enforces the security-model ordering: bounded parsing, stream lookup,
/// authentication, and only then replay-window advancement.
pub struct MediaIngress<A> {
    identity: StreamIdentity,
    authenticator: A,
    replay: ReplayWindow,
}

impl<A: Authenticator> MediaIngress<A> {
    /// Creates ingress state for a freshly negotiated key epoch.
    #[must_use]
    pub const fn new(identity: StreamIdentity, authenticator: A) -> Self {
        Self {
            identity,
            authenticator,
            replay: ReplayWindow::new(),
        }
    }

    /// Returns the exact session, stream, and epoch accepted by this ingress.
    #[must_use]
    pub const fn identity(&self) -> StreamIdentity {
        self.identity
    }

    /// Validates and decrypts a packet into caller-owned storage.
    ///
    /// # Errors
    ///
    /// Returns [`IngressError`] when parsing, stream lookup, authentication, or
    /// authenticated replay validation fails.
    pub fn receive<'a>(
        &mut self,
        datagram: &[u8],
        plaintext: &'a mut [u8],
    ) -> Result<ReceivedFrame<'a>, IngressError<A::Error>> {
        let packet = match parse_media_packet(datagram) {
            Ok(packet) => packet,
            Err(error) => {
                plaintext.fill(0);
                return Err(IngressError::Parse(error));
            }
        };
        if packet.header.session_id != self.identity.session_id
            || packet.header.stream_id != self.identity.stream_id
            || packet.header.key_epoch != self.identity.key_epoch
        {
            plaintext.fill(0);
            return Err(IngressError::UnknownStream);
        }

        let plaintext_len = match self.authenticator.authenticate(
            &packet.header,
            packet.authenticated_header,
            packet.ciphertext,
            plaintext,
        ) {
            Ok(length) => length,
            Err(error) => {
                plaintext.fill(0);
                return Err(IngressError::Authentication(error));
            }
        };
        if plaintext_len > plaintext.len() {
            plaintext.fill(0);
            return Err(IngressError::InvalidPlaintextLength);
        }

        let replay = self.replay.accept_authenticated(packet.header.sequence);
        if replay != ReplayDecision::Accepted {
            plaintext.fill(0);
            return Err(IngressError::Replay(replay));
        }

        Ok(ReceivedFrame {
            header: packet.header,
            opus_payload: &plaintext[..plaintext_len],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonimesh_protocol::MediaHeader;

    #[derive(Debug, Eq, PartialEq)]
    struct TestAuthError;

    impl fmt::Display for TestAuthError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("test authentication failure")
        }
    }

    impl std::error::Error for TestAuthError {}

    struct TestAuthenticator {
        successful_calls: usize,
    }

    struct InvalidLengthAuthenticator {
        calls: usize,
    }

    impl Authenticator for InvalidLengthAuthenticator {
        type Error = TestAuthError;

        fn authenticate(
            &mut self,
            _header: &MediaHeader,
            _authenticated_header: &[u8],
            _ciphertext: &[u8],
            plaintext: &mut [u8],
        ) -> Result<usize, Self::Error> {
            self.calls += 1;
            if self.calls == 1 {
                return Ok(plaintext.len() + 1);
            }
            plaintext[0] = 0xCC;
            Ok(1)
        }
    }

    impl Authenticator for TestAuthenticator {
        type Error = TestAuthError;

        fn authenticate(
            &mut self,
            _header: &MediaHeader,
            _authenticated_header: &[u8],
            ciphertext: &[u8],
            plaintext: &mut [u8],
        ) -> Result<usize, Self::Error> {
            if ciphertext.first() != Some(&0xAA) {
                return Err(TestAuthError);
            }
            self.successful_calls += 1;
            let payload = &ciphertext[1..];
            plaintext[..payload.len()].copy_from_slice(payload);
            Ok(payload.len())
        }
    }

    fn datagram(sequence: u32, ciphertext: &[u8]) -> Vec<u8> {
        let header = MediaHeader {
            flags: 0,
            session_id: 7,
            stream_id: 9,
            sequence,
            sample_timestamp: u64::from(sequence) * 960,
            key_epoch: 1,
        };
        let mut datagram = header.encode(ciphertext.len()).unwrap().to_vec();
        datagram.extend_from_slice(ciphertext);
        datagram
    }

    fn ingress() -> MediaIngress<TestAuthenticator> {
        MediaIngress::new(
            StreamIdentity {
                session_id: 7,
                stream_id: 9,
                key_epoch: 1,
            },
            TestAuthenticator {
                successful_calls: 0,
            },
        )
    }

    #[test]
    fn returns_only_authenticated_plaintext() {
        let mut ingress = ingress();
        let mut plaintext = [0_u8; 32];

        let frame = ingress
            .receive(&datagram(1, &[0xAA, 3, 4]), &mut plaintext)
            .unwrap();

        assert_eq!(frame.opus_payload, [3, 4]);
        assert_eq!(frame.header.sequence, 1);
    }

    #[test]
    fn failed_authentication_does_not_advance_replay_window() {
        let mut ingress = ingress();
        let mut plaintext = [0_u8; 32];

        assert!(matches!(
            ingress.receive(&datagram(5, &[0x00]), &mut plaintext),
            Err(IngressError::Authentication(TestAuthError))
        ));
        assert!(plaintext.iter().all(|byte| *byte == 0));
        assert!(
            ingress
                .receive(&datagram(5, &[0xAA, 7]), &mut plaintext)
                .is_ok()
        );
    }

    #[test]
    fn invalid_plaintext_length_clears_storage_without_advancing_replay() {
        let mut ingress = MediaIngress::new(
            StreamIdentity {
                session_id: 7,
                stream_id: 9,
                key_epoch: 1,
            },
            InvalidLengthAuthenticator { calls: 0 },
        );
        let mut plaintext = [0xFF; 4];

        assert_eq!(
            ingress.receive(&datagram(4, &[0xAA]), &mut plaintext),
            Err(IngressError::InvalidPlaintextLength),
        );
        assert!(plaintext.iter().all(|byte| *byte == 0));

        plaintext.fill(0xFF);
        let frame = ingress
            .receive(&datagram(4, &[0xAA]), &mut plaintext)
            .expect("invalid length must not consume the sequence");
        assert_eq!(frame.opus_payload, [0xCC]);
    }

    #[test]
    fn rejects_authenticated_duplicate_and_clears_reused_plaintext_storage() {
        let mut ingress = ingress();
        let mut plaintext = [0_u8; 32];
        ingress
            .receive(&datagram(2, &[0xAA, 8, 9, 10]), &mut plaintext)
            .unwrap();
        plaintext.fill(0xFF);

        let error = ingress
            .receive(&datagram(2, &[0xAA, 8]), &mut plaintext)
            .unwrap_err();

        assert_eq!(error, IngressError::Replay(ReplayDecision::Duplicate));
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn early_rejections_clear_reused_plaintext_storage() {
        let mut ingress = ingress();
        let mut plaintext = [0xFF; 32];

        assert!(matches!(
            ingress.receive(&[0x00], &mut plaintext),
            Err(IngressError::Parse(PacketParseError::Truncated))
        ));
        assert!(plaintext.iter().all(|byte| *byte == 0));

        let mut unknown = datagram(3, &[0xAA, 1]);
        unknown[8..16].copy_from_slice(&8_u64.to_be_bytes());
        plaintext.fill(0xFF);
        assert_eq!(
            ingress.receive(&unknown, &mut plaintext),
            Err(IngressError::UnknownStream)
        );
        assert!(plaintext.iter().all(|byte| *byte == 0));
        assert_eq!(ingress.authenticator.successful_calls, 0);

        let frame = ingress
            .receive(&datagram(3, &[0xAA, 1]), &mut plaintext)
            .expect("unknown stream must not consume the authentic packet sequence");
        assert_eq!(frame.opus_payload, [1]);
        assert_eq!(ingress.authenticator.successful_calls, 1);
    }
}
