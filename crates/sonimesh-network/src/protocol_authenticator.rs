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

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{
        MediaDirection, MediaHeader, MediaKeyContext, MediaOpener, MediaSealer, SessionSecret,
        parse_media_packet,
    };

    use super::*;

    const PAYLOAD: &[u8] = b"authenticated media payload";

    fn context() -> MediaKeyContext {
        MediaKeyContext::new(7, 9, 1, MediaDirection::AndroidToDesktop)
            .expect("fixed test context is valid")
    }

    fn secret() -> SessionSecret {
        SessionSecret::new([0xA5; 32])
    }

    fn authenticator() -> ProtocolAuthenticator {
        ProtocolAuthenticator::new(
            MediaOpener::new(&secret(), context()).expect("fixed key context derives an opener"),
        )
    }

    fn sealed_datagram(payload: &[u8]) -> Vec<u8> {
        let mut sealer = MediaSealer::new(&secret(), context()).expect("fixed context seals");
        sealer
            .seal(0, 960, payload)
            .expect("bounded test payload is sealable")
            .datagram
    }

    #[test]
    fn authenticates_and_decrypts_a_sealed_packet() {
        let datagram = sealed_datagram(PAYLOAD);
        let packet = parse_media_packet(&datagram).expect("sealed datagram parses");
        let mut plaintext = vec![0_u8; PAYLOAD.len()];

        let written = authenticator()
            .authenticate(
                &packet.header,
                packet.authenticated_header,
                packet.ciphertext,
                &mut plaintext,
            )
            .expect("authentic packet opens");

        assert_eq!(written, PAYLOAD.len());
        assert_eq!(&plaintext[..written], PAYLOAD);
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let datagram = sealed_datagram(PAYLOAD);
        let packet = parse_media_packet(&datagram).expect("sealed datagram parses");
        let mut tampered = packet.ciphertext.to_vec();
        tampered[0] ^= 0x01;
        let mut plaintext = vec![0_u8; PAYLOAD.len()];

        let error = authenticator()
            .authenticate(
                &packet.header,
                packet.authenticated_header,
                &tampered,
                &mut plaintext,
            )
            .expect_err("tampered ciphertext must fail authentication");

        assert!(matches!(
            error,
            sonimesh_protocol::MediaCryptoError::AuthenticationFailed
        ));
    }

    #[test]
    fn rejects_a_header_from_a_different_context() {
        let datagram = sealed_datagram(PAYLOAD);
        let packet = parse_media_packet(&datagram).expect("sealed datagram parses");
        let wrong_header = MediaHeader {
            stream_id: 99,
            ..packet.header
        };
        let mut plaintext = vec![0_u8; PAYLOAD.len()];

        let error = authenticator()
            .authenticate(
                &wrong_header,
                packet.authenticated_header,
                packet.ciphertext,
                &mut plaintext,
            )
            .expect_err("wrong-context header must fail");

        assert!(matches!(
            error,
            sonimesh_protocol::MediaCryptoError::ContextMismatch
        ));
    }
}
