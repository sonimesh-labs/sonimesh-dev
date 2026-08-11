use core::fmt;

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

use crate::MEDIA_VERSION;

const ROOT_SALT: &[u8] = b"sonimesh media root v1";
const STREAM_KEY_LABEL: &[u8] = b"sonimesh media stream key v1";

/// Direction bound into a media key and packet nonce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MediaDirection {
    /// Android sender to desktop receiver.
    AndroidToDesktop = 0,
    /// Desktop sender to Android receiver.
    DesktopToAndroid = 1,
}

/// Media fields that select a unique per-stream AEAD key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaKeyContext {
    /// Random media-session identifier bound into HKDF info and packet headers.
    pub session_id: u64,
    /// Stream identifier bound into HKDF info and packet nonces.
    pub stream_id: u32,
    /// Explicit key-rotation generation.
    pub key_epoch: u16,
    /// Packet direction.
    pub direction: MediaDirection,
}

impl MediaKeyContext {
    /// Creates a non-zero media key context.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::InvalidContext`] when session or stream ID is
    /// zero, which is reserved as an invalid media context.
    pub const fn new(
        session_id: u64,
        stream_id: u32,
        key_epoch: u16,
        direction: MediaDirection,
    ) -> Result<Self, MediaCryptoError> {
        if session_id == 0 || stream_id == 0 {
            return Err(MediaCryptoError::InvalidContext);
        }
        Ok(Self {
            session_id,
            stream_id,
            key_epoch,
            direction,
        })
    }
}

/// Opaque 256-bit root secret from an authenticated control-session transcript.
pub struct SessionSecret([u8; 32]);

impl SessionSecret {
    /// Wraps exactly 32 secret bytes supplied by an authenticated session setup.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Drop for SessionSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SessionSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionSecret([REDACTED])")
    }
}

/// Sender-owned sequence source that refuses reuse after `u32::MAX`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSequence {
    next: Option<u32>,
}

impl MediaSequence {
    /// Starts a fresh epoch at `initial` sequence.
    #[must_use]
    pub const fn new(initial: u32) -> Self {
        Self {
            next: Some(initial),
        }
    }

    /// Returns a unique sequence or reports that the epoch requires rotation.
    ///
    /// # Errors
    ///
    /// Returns [`MediaCryptoError::SequenceExhausted`] after `u32::MAX` has
    /// already been issued.
    pub fn take_next(&mut self) -> Result<u32, MediaCryptoError> {
        let sequence = self.next.ok_or(MediaCryptoError::SequenceExhausted)?;
        self.next = sequence.checked_add(1);
        Ok(sequence)
    }
}

/// Media encryption/decryption failure that never exposes secret material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCryptoError {
    /// Session or stream identifier is zero.
    InvalidContext,
    /// Sender sequence reached the mandatory key-rotation boundary.
    SequenceExhausted,
    /// Packet header does not match the opener's key context.
    ContextMismatch,
    /// Plaintext/ciphertext exceeded the bounded media payload capacity.
    PayloadTooLarge,
    /// Packet lacks the 16-byte Poly1305 authentication tag.
    TruncatedTag,
    /// Caller-owned plaintext output is too small.
    OutputTooSmall,
    /// AEAD verification failed.
    AuthenticationFailed,
    /// HKDF cannot expand the requested media key material.
    KeyDerivationFailed,
}

impl fmt::Display for MediaCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidContext => formatter.write_str("media key context is invalid"),
            Self::SequenceExhausted => formatter.write_str("media key epoch sequence is exhausted"),
            Self::ContextMismatch => formatter.write_str("media packet key context does not match"),
            Self::PayloadTooLarge => formatter.write_str("media payload exceeds the hard limit"),
            Self::TruncatedTag => {
                formatter.write_str("media ciphertext lacks an authentication tag")
            }
            Self::OutputTooSmall => formatter.write_str("media plaintext output is too small"),
            Self::AuthenticationFailed => formatter.write_str("media authentication failed"),
            Self::KeyDerivationFailed => formatter.write_str("media key derivation failed"),
        }
    }
}

impl std::error::Error for MediaCryptoError {}

pub(crate) fn derive_media_key(
    secret: &SessionSecret,
    context: MediaKeyContext,
) -> Result<[u8; 32], MediaCryptoError> {
    let mut info = [0_u8; STREAM_KEY_LABEL.len() + 1 + 8 + 4 + 2 + 1];
    let mut offset = 0;
    info[offset..offset + STREAM_KEY_LABEL.len()].copy_from_slice(STREAM_KEY_LABEL);
    offset += STREAM_KEY_LABEL.len();
    info[offset] = MEDIA_VERSION;
    offset += 1;
    info[offset..offset + 8].copy_from_slice(&context.session_id.to_be_bytes());
    offset += 8;
    info[offset..offset + 4].copy_from_slice(&context.stream_id.to_be_bytes());
    offset += 4;
    info[offset..offset + 2].copy_from_slice(&context.key_epoch.to_be_bytes());
    offset += 2;
    info[offset] = context.direction as u8;

    let hkdf = Hkdf::<Sha256>::new(Some(ROOT_SALT), secret.as_bytes());
    let mut key = [0_u8; 32];
    hkdf.expand(&info, &mut key)
        .map_err(|_| MediaCryptoError::KeyDerivationFailed)?;
    Ok(key)
}

/// Builds the exact 96-bit ChaCha20-Poly1305 nonce for one packet sequence.
#[must_use]
pub const fn media_nonce(context: MediaKeyContext, sequence: u32) -> [u8; 12] {
    let epoch = context.key_epoch.to_be_bytes();
    let stream = context.stream_id.to_be_bytes();
    let sequence = sequence.to_be_bytes();
    [
        epoch[0],
        epoch[1],
        context.direction as u8,
        0,
        stream[0],
        stream[1],
        stream[2],
        stream[3],
        sequence[0],
        sequence[1],
        sequence[2],
        sequence[3],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(direction: MediaDirection) -> MediaKeyContext {
        MediaKeyContext::new(7, 9, 3, direction).unwrap()
    }

    #[test]
    fn derives_deterministically_and_separates_direction() {
        let secret = SessionSecret::new([4; 32]);
        let forward = derive_media_key(&secret, context(MediaDirection::AndroidToDesktop)).unwrap();
        let same = derive_media_key(&secret, context(MediaDirection::AndroidToDesktop)).unwrap();
        let reverse = derive_media_key(&secret, context(MediaDirection::DesktopToAndroid)).unwrap();

        assert_eq!(forward, same);
        assert_ne!(forward, reverse);
    }

    #[test]
    fn nonce_changes_for_epoch_stream_direction_and_sequence() {
        let base = context(MediaDirection::AndroidToDesktop);
        let mut changed_stream = base;
        changed_stream.stream_id += 1;
        let mut changed_epoch = base;
        changed_epoch.key_epoch += 1;

        assert_ne!(media_nonce(base, 1), media_nonce(base, 2));
        assert_ne!(media_nonce(base, 1), media_nonce(changed_stream, 1));
        assert_ne!(media_nonce(base, 1), media_nonce(changed_epoch, 1));
        assert_ne!(
            media_nonce(base, 1),
            media_nonce(context(MediaDirection::DesktopToAndroid), 1)
        );
    }

    #[test]
    fn nonce_binds_each_dimension_without_byte_overlap() {
        let base = context(MediaDirection::AndroidToDesktop);

        // The epoch occupies exactly bytes 0..2; a full sweep stays unique and
        // leaves every other byte fixed, so the epoch dimension never truncates.
        let mut epoch_fixed = [0_u8; 10];
        epoch_fixed.copy_from_slice(&media_nonce(base, 1)[2..]);
        let mut epochs = std::collections::HashSet::new();
        for epoch in 0..=u16::MAX {
            let mut changed = base;
            changed.key_epoch = epoch;
            let nonce = media_nonce(changed, 1);
            assert_eq!(nonce[0..2], epoch.to_be_bytes());
            assert_eq!(nonce[2..], epoch_fixed);
            epochs.insert(nonce);
        }
        assert_eq!(epochs.len(), usize::from(u16::MAX) + 1);

        // The stream occupies exactly bytes 4..8 without leaking into neighbors.
        let mut stream_fixed = [0_u8; 8];
        stream_fixed[0..4].copy_from_slice(&media_nonce(base, 1)[0..4]);
        stream_fixed[4..8].copy_from_slice(&media_nonce(base, 1)[8..12]);
        for stream in [1_u32, 2, 0x00FF_FFFF, 0x0100_0000, u32::MAX - 1, u32::MAX] {
            let mut changed = base;
            changed.stream_id = stream;
            let nonce = media_nonce(changed, 1);
            assert_eq!(nonce[4..8], stream.to_be_bytes());
            assert_eq!(nonce[0..4], stream_fixed[0..4]);
            assert_eq!(nonce[8..12], stream_fixed[4..8]);
        }

        // The sequence occupies exactly bytes 8..12 without leaking into neighbors.
        let mut sequence_fixed = [0_u8; 8];
        sequence_fixed.copy_from_slice(&media_nonce(base, 0)[0..8]);
        for sequence in [0_u32, 1, 0x00FF_FFFF, 0x0100_0000, u32::MAX - 1, u32::MAX] {
            let nonce = media_nonce(base, sequence);
            assert_eq!(nonce[8..12], sequence.to_be_bytes());
            assert_eq!(nonce[0..8], sequence_fixed);
        }

        // The direction occupies exactly byte 2 with a reserved zero padding byte.
        let forward = media_nonce(base, 1);
        let reverse = media_nonce(context(MediaDirection::DesktopToAndroid), 1);
        assert_eq!(forward[2], MediaDirection::AndroidToDesktop as u8);
        assert_eq!(reverse[2], MediaDirection::DesktopToAndroid as u8);
        assert_eq!(forward[3], 0);
        assert_eq!(reverse[3], 0);
        assert_eq!(forward[0..2], reverse[0..2]);
        assert_eq!(forward[4..12], reverse[4..12]);
    }

    #[test]
    fn sequence_requires_rotation_after_maximum_value() {
        let mut sequence = MediaSequence::new(u32::MAX);
        assert_eq!(sequence.take_next(), Ok(u32::MAX));
        assert_eq!(
            sequence.take_next(),
            Err(MediaCryptoError::SequenceExhausted)
        );
    }
}
