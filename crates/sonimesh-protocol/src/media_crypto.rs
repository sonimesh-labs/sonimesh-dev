use core::fmt;

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, Tag};
use zeroize::Zeroize;

use crate::media_key::derive_media_key;
use crate::{
    MAX_ENCRYPTED_PAYLOAD_LEN, MEDIA_HEADER_LEN, MediaCryptoError, MediaHeader, MediaKeyContext,
    MediaPacket, MediaSequence, PacketParseError, SessionSecret, media_nonce,
};

const POLY1305_TAG_BYTES: usize = 16;
/// Largest plaintext accepted before Poly1305 tag expansion.
pub const MAX_MEDIA_PLAINTEXT_LEN: usize = MAX_ENCRYPTED_PAYLOAD_LEN - POLY1305_TAG_BYTES;

/// One authenticated media packet produced by [`MediaSealer`].
#[derive(Clone, Eq, PartialEq)]
pub struct ProtectedMediaPacket {
    /// Parsed fixed header fields used to create the packet.
    pub header: MediaHeader,
    /// Complete encoded datagram: header followed by ciphertext and tag.
    pub datagram: Vec<u8>,
}

impl fmt::Debug for ProtectedMediaPacket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtectedMediaPacket")
            .field("header", &self.header)
            .field("datagram_len", &self.datagram.len())
            .finish()
    }
}

/// Sender-side state that guarantees a sequence is not reused within one epoch.
pub struct MediaSealer {
    context: MediaKeyContext,
    cipher: ChaCha20Poly1305,
    sequence: MediaSequence,
}

impl MediaSealer {
    /// Derives a sender key and starts a fresh sequence at zero.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::KeyDerivationFailed`] only when HKDF expansion
    /// cannot produce the fixed-size ChaCha20 key.
    pub fn new(secret: &SessionSecret, context: MediaKeyContext) -> Result<Self, MediaCryptoError> {
        let mut key = derive_media_key(secret, context)?;
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| MediaCryptoError::KeyDerivationFailed)?;
        key.zeroize();
        Ok(Self {
            context,
            cipher,
            sequence: MediaSequence::new(0),
        })
    }

    /// Encrypts one bounded payload and authenticates the complete media header.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError`] for oversized payloads, sequence exhaustion,
    /// or AEAD setup failure. A new key epoch is required after exhaustion.
    pub fn seal(
        &mut self,
        flags: u8,
        sample_timestamp: u64,
        plaintext: &[u8],
    ) -> Result<ProtectedMediaPacket, MediaCryptoError> {
        if plaintext.len() > MAX_MEDIA_PLAINTEXT_LEN {
            return Err(MediaCryptoError::PayloadTooLarge);
        }
        let sequence = self.sequence.take_next()?;
        let header = MediaHeader {
            flags,
            session_id: self.context.session_id,
            stream_id: self.context.stream_id,
            sequence,
            sample_timestamp,
            key_epoch: self.context.key_epoch,
        };
        let header_bytes = header
            .encode(plaintext.len() + POLY1305_TAG_BYTES)
            .map_err(map_packet_error)?;
        let mut ciphertext = plaintext.to_vec();
        let nonce_bytes = media_nonce(self.context, sequence);
        let nonce = Nonce::try_from(nonce_bytes.as_slice())
            .map_err(|_| MediaCryptoError::AuthenticationFailed)?;
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce, &header_bytes, ciphertext.as_mut_slice().into())
            .map_err(|_| MediaCryptoError::AuthenticationFailed)?;
        ciphertext.extend_from_slice(&tag);

        let mut datagram = Vec::with_capacity(MEDIA_HEADER_LEN + ciphertext.len());
        datagram.extend_from_slice(&header_bytes);
        datagram.extend_from_slice(&ciphertext);
        Ok(ProtectedMediaPacket { header, datagram })
    }
}

/// Receiver-side AEAD context for one session, stream, direction, and epoch.
pub struct MediaOpener {
    context: MediaKeyContext,
    cipher: ChaCha20Poly1305,
}

impl MediaOpener {
    /// Derives a receiver key for the supplied media context.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::KeyDerivationFailed`] only when HKDF expansion
    /// cannot produce the fixed-size ChaCha20 key.
    pub fn new(secret: &SessionSecret, context: MediaKeyContext) -> Result<Self, MediaCryptoError> {
        let mut key = derive_media_key(secret, context)?;
        let cipher = ChaCha20Poly1305::new_from_slice(&key)
            .map_err(|_| MediaCryptoError::KeyDerivationFailed)?;
        key.zeroize();
        Ok(Self { context, cipher })
    }

    /// Authenticates and decrypts a parsed media packet into caller-owned storage.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError`] for context mismatch, tag truncation, output
    /// bounds, or failed authentication. Authentication failure zeroes the output
    /// region that temporarily held ciphertext.
    pub fn open(
        &self,
        packet: MediaPacket<'_>,
        plaintext: &mut [u8],
    ) -> Result<usize, MediaCryptoError> {
        if packet.header.session_id != self.context.session_id
            || packet.header.stream_id != self.context.stream_id
            || packet.header.key_epoch != self.context.key_epoch
        {
            return Err(MediaCryptoError::ContextMismatch);
        }
        if packet.ciphertext.len() < POLY1305_TAG_BYTES {
            return Err(MediaCryptoError::TruncatedTag);
        }
        let ciphertext_len = packet.ciphertext.len() - POLY1305_TAG_BYTES;
        if ciphertext_len > plaintext.len() {
            return Err(MediaCryptoError::OutputTooSmall);
        }
        plaintext[..ciphertext_len].copy_from_slice(&packet.ciphertext[..ciphertext_len]);
        let nonce_bytes = media_nonce(self.context, packet.header.sequence);
        let nonce = Nonce::try_from(nonce_bytes.as_slice())
            .map_err(|_| MediaCryptoError::AuthenticationFailed)?;
        let tag = Tag::try_from(&packet.ciphertext[ciphertext_len..])
            .map_err(|_| MediaCryptoError::TruncatedTag)?;
        if self
            .cipher
            .decrypt_inout_detached(
                &nonce,
                packet.authenticated_header,
                (&mut plaintext[..ciphertext_len]).into(),
                &tag,
            )
            .is_err()
        {
            plaintext[..ciphertext_len].zeroize();
            return Err(MediaCryptoError::AuthenticationFailed);
        }
        Ok(ciphertext_len)
    }
}

fn map_packet_error(error: PacketParseError) -> MediaCryptoError {
    match error {
        PacketParseError::PayloadTooLarge | PacketParseError::DatagramTooLarge => {
            MediaCryptoError::PayloadTooLarge
        }
        PacketParseError::Truncated
        | PacketParseError::InvalidMagic
        | PacketParseError::UnsupportedVersion(_)
        | PacketParseError::InvalidHeaderLength
        | PacketParseError::LengthMismatch => MediaCryptoError::PayloadTooLarge,
    }
}

#[cfg(test)]
mod debug_tests {
    use crate::{MediaHeader, ProtectedMediaPacket};

    #[test]
    fn protected_packet_debug_omits_complete_datagram_bytes() {
        let packet = ProtectedMediaPacket {
            header: MediaHeader {
                flags: 0,
                session_id: 7,
                stream_id: 9,
                sequence: 3,
                sample_timestamp: 2_880,
                key_epoch: 1,
            },
            datagram: b"complete protected datagram".to_vec(),
        };

        let debug = format!("{packet:?}");

        assert!(debug.contains("datagram_len: 27"));
        assert!(!debug.contains("datagram:"));
        assert!(!debug.contains("99, 111, 109"));
    }
}
