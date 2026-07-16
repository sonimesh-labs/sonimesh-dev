use sonimesh_protocol::{MediaCryptoError, MediaHeader, MediaOpener, MediaPacket};

use crate::Authenticator;

/// Network adapter that exposes the reviewed protocol AEAD opener to ingress.
pub struct ProtocolAuthenticator {
    opener: MediaOpener,
}

impl ProtocolAuthenticator {
    /// Creates an adapter for one authenticated session, stream, direction, and epoch.
    #[must_use]
    pub const fn new(opener: MediaOpener) -> Self {
        Self { opener }
    }
}

impl Authenticator for ProtocolAuthenticator {
    type Error = MediaCryptoError;

    fn authenticate(
        &mut self,
        header: &MediaHeader,
        authenticated_header: &[u8],
        ciphertext: &[u8],
        plaintext: &mut [u8],
    ) -> Result<usize, Self::Error> {
        self.opener.open(
            MediaPacket {
                header: *header,
                authenticated_header,
                ciphertext,
            },
            plaintext,
        )
    }
}
