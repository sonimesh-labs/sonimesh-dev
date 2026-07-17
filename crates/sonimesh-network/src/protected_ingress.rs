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
    /// A fresh-session installation reused the current media session ID.
    FreshSessionRequired,
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
            Self::FreshSessionRequired => {
                formatter.write_str("fresh media installation must use a new session ID")
            }
        }
    }
}

impl std::error::Error for ProtectedIngressError {}

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
