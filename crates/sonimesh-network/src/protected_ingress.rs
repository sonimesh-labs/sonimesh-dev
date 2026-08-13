use core::fmt;

use sonimesh_protocol::{MediaCryptoError, MediaKeyContext, MediaOpener, SessionSecret};

use crate::{IngressError, MediaIngress, ProtocolAuthenticator, ReceivedFrame, StreamIdentity};

/// Error while creating or rotating one AEAD-protected media ingress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedIngressError {
    /// Media key derivation or opener creation failed.
    Crypto(MediaCryptoError),
    /// Rotation changed session, stream, or direction instead of only advancing epoch.
    RotationContextMismatch,
    /// Rotation did not strictly advance the 16-bit key epoch.
    NonIncreasingEpoch,
}

impl fmt::Display for ProtectedIngressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crypto(error) => write!(formatter, "protected ingress setup failed: {error}"),
            Self::RotationContextMismatch => {
                formatter.write_str("media rotation changed the session, stream, or direction")
            }
            Self::NonIncreasingEpoch => {
                formatter.write_str("media rotation key epoch must strictly increase")
            }
        }
    }
}

impl std::error::Error for ProtectedIngressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Crypto(error) => Some(error),
            Self::RotationContextMismatch | Self::NonIncreasingEpoch => None,
        }
    }
}

/// Network-domain owner for one AEAD opener and its matching replay window.
///
/// The caller obtains the root secret from an authenticated control session.
/// This type never stores that root secret; it derives an opener for the current
/// context, and a successful rotation atomically replaces the entire ingress.
pub struct ProtectedMediaIngress {
    context: MediaKeyContext,
    ingress: MediaIngress<ProtocolAuthenticator>,
}

impl ProtectedMediaIngress {
    /// Builds fresh AEAD and replay state for one accepted media key context.
    ///
    /// # Errors
    ///
    /// Returns [`ProtectedIngressError::Crypto`] when the protocol opener cannot
    /// derive the required key material.
    pub fn new(
        secret: &SessionSecret,
        context: MediaKeyContext,
    ) -> Result<Self, ProtectedIngressError> {
        let opener = MediaOpener::new(secret, context).map_err(ProtectedIngressError::Crypto)?;
        Ok(Self {
            context,
            ingress: MediaIngress::new(identity_for(context), ProtocolAuthenticator::new(opener)),
        })
    }

    /// Returns the media key context currently installed for packet admission.
    #[must_use]
    pub const fn context(&self) -> MediaKeyContext {
        self.context
    }

    /// Returns the stream identity currently installed for packet admission.
    #[must_use]
    pub const fn identity(&self) -> StreamIdentity {
        self.ingress.identity()
    }

    /// Replaces the opener and replay window with a strictly newer key epoch.
    ///
    /// # Errors
    ///
    /// Returns an error without changing current ingress state when the context
    /// changes session, stream, or direction; fails to advance epoch; or cannot
    /// derive the next opener.
    pub fn rotate(
        &mut self,
        secret: &SessionSecret,
        next: MediaKeyContext,
    ) -> Result<(), ProtectedIngressError> {
        if next.session_id != self.context.session_id
            || next.stream_id != self.context.stream_id
            || next.direction != self.context.direction
        {
            return Err(ProtectedIngressError::RotationContextMismatch);
        }
        if next.key_epoch <= self.context.key_epoch {
            return Err(ProtectedIngressError::NonIncreasingEpoch);
        }

        let replacement = Self::new(secret, next)?;
        *self = replacement;
        Ok(())
    }

    /// Validates and decrypts one packet into caller-owned plaintext storage.
    ///
    /// # Errors
    ///
    /// Returns parsing, context, authentication, or replay failures before an
    /// Opus payload can reach the per-stream decode worker.
    pub fn receive<'a>(
        &mut self,
        datagram: &[u8],
        plaintext: &'a mut [u8],
    ) -> Result<ReceivedFrame<'a>, IngressError<MediaCryptoError>> {
        self.ingress.receive(datagram, plaintext)
    }
}

const fn identity_for(context: MediaKeyContext) -> StreamIdentity {
    StreamIdentity {
        session_id: context.session_id,
        stream_id: context.stream_id,
        key_epoch: context.key_epoch,
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{
        MAX_MEDIA_PLAINTEXT_LEN, MediaDirection, MediaKeyContext, MediaSealer, PacketParseError,
        ReplayDecision, SessionSecret,
    };

    use super::{ProtectedIngressError, ProtectedMediaIngress};
    use crate::{IngressError, StreamIdentity};

    const SESSION_ID: u64 = 7;
    const STREAM_ID: u32 = 9;
    const EPOCH: u16 = 1;

    fn context(key_epoch: u16) -> MediaKeyContext {
        MediaKeyContext::new(
            SESSION_ID,
            STREAM_ID,
            key_epoch,
            MediaDirection::AndroidToDesktop,
        )
        .expect("fixed test context is valid")
    }

    fn secret() -> SessionSecret {
        SessionSecret::new([0xA5; 32])
    }

    fn ingress(key_epoch: u16) -> ProtectedMediaIngress {
        ProtectedMediaIngress::new(&secret(), context(key_epoch))
            .expect("fixed test key context derives an opener")
    }

    fn sealed_datagram(context: MediaKeyContext, payload: &[u8]) -> Vec<u8> {
        let mut sealer = MediaSealer::new(&secret(), context).expect("fixed test context seals");
        sealer
            .seal(0, 960, payload)
            .expect("bounded test payload is sealable")
            .datagram
    }

    #[test]
    fn ingress_exposes_the_installed_context_and_identity() {
        let ingress = ingress(EPOCH);

        assert_eq!(ingress.context(), context(EPOCH));
        assert_eq!(
            ingress.identity(),
            StreamIdentity {
                session_id: SESSION_ID,
                stream_id: STREAM_ID,
                key_epoch: EPOCH,
            }
        );
    }

    #[test]
    fn receive_round_trips_a_sealed_payload() {
        let mut ingress = ingress(EPOCH);
        let datagram = sealed_datagram(context(EPOCH), b"opus frame bytes");
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];

        let frame = ingress.receive(&datagram, &mut plaintext).unwrap();

        assert_eq!(frame.opus_payload, b"opus frame bytes");
        assert_eq!(frame.header.session_id, SESSION_ID);
        assert_eq!(frame.header.stream_id, STREAM_ID);
        assert_eq!(frame.header.key_epoch, EPOCH);
        assert_eq!(frame.header.sequence, 0);
    }

    #[test]
    fn rejects_an_authenticated_replay_and_clears_plaintext() {
        let mut ingress = ingress(EPOCH);
        let datagram = sealed_datagram(context(EPOCH), &[1, 2, 3]);
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        ingress.receive(&datagram, &mut plaintext).unwrap();
        plaintext.fill(0xA5);

        let error = ingress.receive(&datagram, &mut plaintext).unwrap_err();

        assert_eq!(error, IngressError::Replay(ReplayDecision::Duplicate));
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rejects_a_malformed_datagram_before_authentication() {
        let mut ingress = ingress(EPOCH);
        let mut plaintext = [0xFF; 8];

        assert_eq!(
            ingress.receive(&[0x00], &mut plaintext),
            Err(IngressError::Parse(PacketParseError::Truncated))
        );
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rejects_tampered_ciphertext_and_clears_plaintext() {
        let mut ingress = ingress(EPOCH);
        let mut datagram = sealed_datagram(context(EPOCH), &[9, 8, 7]);
        let last = datagram.len() - 1;
        datagram[last] ^= 0x01;
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];

        assert!(matches!(
            ingress.receive(&datagram, &mut plaintext),
            Err(IngressError::Authentication(_))
        ));
        assert!(plaintext.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rejects_a_plaintext_buffer_shorter_than_the_payload() {
        let mut ingress = ingress(EPOCH);
        let payload = [0xAB; 16];
        let datagram = sealed_datagram(context(EPOCH), &payload);
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        let short = payload.len() - 1;

        assert!(ingress.receive(&datagram, &mut plaintext[..short]).is_err());
        assert!(plaintext[..short].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rotation_rejects_a_changed_session_stream_or_direction() {
        let mut ingress = ingress(EPOCH);
        let datagram = sealed_datagram(context(EPOCH), &[1]);

        let mut changed_session = context(2);
        changed_session.session_id += 1;
        assert_eq!(
            ingress.rotate(&secret(), changed_session),
            Err(ProtectedIngressError::RotationContextMismatch)
        );

        let mut changed_stream = context(2);
        changed_stream.stream_id += 1;
        assert_eq!(
            ingress.rotate(&secret(), changed_stream),
            Err(ProtectedIngressError::RotationContextMismatch)
        );

        let changed_direction =
            MediaKeyContext::new(SESSION_ID, STREAM_ID, 2, MediaDirection::DesktopToAndroid)
                .expect("fixed direction variant is valid");
        assert_eq!(
            ingress.rotate(&secret(), changed_direction),
            Err(ProtectedIngressError::RotationContextMismatch)
        );

        assert_eq!(ingress.context(), context(EPOCH));
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        assert!(ingress.receive(&datagram, &mut plaintext).is_ok());
    }

    #[test]
    fn rotation_rejects_a_non_increasing_epoch() {
        let mut ingress = ingress(3);

        assert_eq!(
            ingress.rotate(&secret(), context(3)),
            Err(ProtectedIngressError::NonIncreasingEpoch)
        );
        assert_eq!(
            ingress.rotate(&secret(), context(2)),
            Err(ProtectedIngressError::NonIncreasingEpoch)
        );
        assert_eq!(ingress.context(), context(3));
    }

    #[test]
    fn rotation_advances_the_epoch_and_replaces_the_opener() {
        let mut ingress = ingress(EPOCH);
        let old_datagram = sealed_datagram(context(EPOCH), &[1]);

        ingress.rotate(&secret(), context(2)).unwrap();
        assert_eq!(ingress.context(), context(2));
        assert_eq!(ingress.identity().key_epoch, 2);

        // The previous epoch is no longer admitted by the replacement ingress.
        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        assert_eq!(
            ingress.receive(&old_datagram, &mut plaintext),
            Err(IngressError::UnknownStream)
        );
        assert!(plaintext.iter().all(|byte| *byte == 0));

        // The new epoch decrypts under the replacement opener.
        let new_datagram = sealed_datagram(context(2), &[2, 3]);
        let frame = ingress.receive(&new_datagram, &mut plaintext).unwrap();
        assert_eq!(frame.opus_payload, [2, 3]);
    }

    #[test]
    fn rotation_failure_keeps_the_previous_epoch_working() {
        let mut ingress = ingress(EPOCH);
        let datagram = sealed_datagram(context(EPOCH), &[7]);

        assert!(ingress.rotate(&secret(), context(EPOCH)).is_err());

        let mut plaintext = [0xA5; MAX_MEDIA_PLAINTEXT_LEN];
        let frame = ingress.receive(&datagram, &mut plaintext).unwrap();
        assert_eq!(frame.opus_payload, [7]);
    }

    #[test]
    fn protected_ingress_error_display_is_actionable() {
        assert_eq!(
            ProtectedIngressError::RotationContextMismatch.to_string(),
            "media rotation changed the session, stream, or direction"
        );
        assert_eq!(
            ProtectedIngressError::NonIncreasingEpoch.to_string(),
            "media rotation key epoch must strictly increase"
        );
    }
}
