use core::fmt;

use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::{CONTROL_PROTOCOL_VERSION, SessionSecret};

/// TLS exporter byte length required by the version-one pairing binding.
pub const PAIRING_TLS_EXPORTER_LEN: usize = 32;
/// TLS exporter label required by the version-one pairing binding.
pub const PAIRING_TLS_EXPORTER_LABEL: &[u8] = b"EXPORTER-SoniMesh Pairing v1";

const PAIRING_ID_LEN: usize = 16;
const PAIRING_SECRET_LEN: usize = 32;
const PROOF_LABEL: &[u8] = b"sonimesh pairing proof v1";
const MEDIA_ROOT_LABEL: &[u8] = b"sonimesh pairing media root v1";

/// Role bound into a version-one pairing proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PairingRole {
    /// Android establishes the TLS connection and proves the client role.
    AndroidClient = 0,
    /// Desktop accepts the TLS connection and proves the server role.
    DesktopServer = 1,
}

impl PairingRole {
    const fn peer(self) -> Self {
        match self {
            Self::AndroidClient => Self::DesktopServer,
            Self::DesktopServer => Self::AndroidClient,
        }
    }
}

/// Error deriving a pairing proof or media root from TLS exporter bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingBindingError {
    /// The pairing binding did not use the current control protocol version.
    InvalidProtocolVersion,
    /// The caller did not provide the exact required TLS exporter length.
    InvalidExporterLength,
    /// HKDF could not expand the required fixed-length output.
    DerivationFailed,
}

impl fmt::Display for PairingBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProtocolVersion => {
                formatter.write_str("pairing binding protocol version is invalid")
            }
            Self::InvalidExporterLength => {
                formatter.write_str("pairing TLS exporter length is invalid")
            }
            Self::DerivationFailed => formatter.write_str("pairing binding derivation failed"),
        }
    }
}

impl std::error::Error for PairingBindingError {}

/// Pure pairing binding derived from externally authenticated TLS exporter bytes.
///
/// This type neither creates a TLS connection nor validates a certificate pin.
/// A platform pairing owner may construct it only after it has completed the
/// exact TLS 1.3 exporter call defined in ADR 0008.
pub struct PairingBinding {
    prk: [u8; PAIRING_SECRET_LEN],
    pairing_id: [u8; PAIRING_ID_LEN],
    protocol_version: u8,
}

impl PairingBinding {
    /// Derives a binding from one QR pairing ID, its secret, and TLS exporter bytes.
    ///
    /// # Errors
    ///
    /// Returns [`PairingBindingError::InvalidExporterLength`] unless the caller
    /// supplies exactly the 32 bytes required by the TLS exporter contract.
    pub fn new(
        protocol_version: u8,
        pairing_id: [u8; PAIRING_ID_LEN],
        pairing_secret: [u8; PAIRING_SECRET_LEN],
        tls_exporter: &[u8],
    ) -> Result<Self, PairingBindingError> {
        let pairing_secret = Zeroizing::new(pairing_secret);
        if u32::from(protocol_version) != CONTROL_PROTOCOL_VERSION {
            return Err(PairingBindingError::InvalidProtocolVersion);
        }
        let exporter = Zeroizing::new(
            tls_exporter
                .try_into()
                .map_err(|_| PairingBindingError::InvalidExporterLength)?,
        );
        let prk = hmac_extract(&pairing_secret, &exporter);
        Ok(Self {
            prk,
            pairing_id,
            protocol_version,
        })
    }

    /// Derives the proof this endpoint sends for its `local_role`.
    ///
    /// The pairing owner supplies only its own role. The opposite role remains
    /// an internal verification detail, preventing a call site from accepting
    /// its own proof as a peer proof by selecting an arbitrary expected role.
    ///
    /// # Errors
    ///
    /// Returns [`PairingBindingError::DerivationFailed`] if the fixed HKDF
    /// proof expansion cannot produce its required 32 bytes.
    pub fn local_proof(
        &self,
        local_role: PairingRole,
    ) -> Result<[u8; PAIRING_SECRET_LEN], PairingBindingError> {
        self.proof_for_role(local_role)
    }

    /// Verifies a peer proof for this endpoint's `local_role`.
    ///
    /// The proof is checked against the opposite role in constant time after
    /// rejecting non-canonical proof lengths. A reflected local proof therefore
    /// cannot verify as a peer proof.
    ///
    /// # Errors
    ///
    /// Returns [`PairingBindingError::DerivationFailed`] if the fixed HKDF
    /// proof expansion cannot produce its required 32 bytes.
    pub fn verify_peer_proof(
        &self,
        local_role: PairingRole,
        proof: &[u8],
    ) -> Result<bool, PairingBindingError> {
        let proof: &[u8; PAIRING_SECRET_LEN] = match proof.try_into() {
            Ok(proof) => proof,
            Err(_) => return Ok(false),
        };
        let mut expected = self.proof_for_role(local_role.peer())?;
        let mut difference = 0_u8;
        for (expected_byte, proof_byte) in expected.iter().zip(proof) {
            difference |= expected_byte ^ proof_byte;
        }
        expected.zeroize();
        Ok(difference == 0)
    }

    fn proof_for_role(
        &self,
        role: PairingRole,
    ) -> Result<[u8; PAIRING_SECRET_LEN], PairingBindingError> {
        let mut info = [0_u8; PROOF_LABEL.len() + 1 + 1 + PAIRING_ID_LEN];
        let mut offset = 0;
        info[offset..offset + PROOF_LABEL.len()].copy_from_slice(PROOF_LABEL);
        offset += PROOF_LABEL.len();
        info[offset] = self.protocol_version;
        offset += 1;
        info[offset] = role as u8;
        offset += 1;
        info[offset..].copy_from_slice(&self.pairing_id);
        hkdf_expand(&self.prk, &info)
    }

    /// Derives the one-time media root passed to the existing media-key boundary.
    ///
    /// # Errors
    ///
    /// Returns [`PairingBindingError::DerivationFailed`] if the fixed HKDF
    /// media-root expansion cannot produce its required 32 bytes.
    pub fn media_root(&self) -> Result<SessionSecret, PairingBindingError> {
        let mut info = [0_u8; MEDIA_ROOT_LABEL.len() + 1 + PAIRING_ID_LEN];
        let mut offset = 0;
        info[offset..offset + MEDIA_ROOT_LABEL.len()].copy_from_slice(MEDIA_ROOT_LABEL);
        offset += MEDIA_ROOT_LABEL.len();
        info[offset] = self.protocol_version;
        offset += 1;
        info[offset..].copy_from_slice(&self.pairing_id);
        Ok(SessionSecret::new(hkdf_expand(&self.prk, &info)?))
    }
}

impl Drop for PairingBinding {
    fn drop(&mut self) {
        self.prk.zeroize();
    }
}

impl fmt::Debug for PairingBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingBinding([REDACTED])")
    }
}

fn hmac_extract(
    salt: &[u8; PAIRING_SECRET_LEN],
    ikm: &[u8; PAIRING_TLS_EXPORTER_LEN],
) -> [u8; PAIRING_SECRET_LEN] {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(salt).expect("HMAC accepts a fixed-size key");
    mac.update(ikm);
    mac.finalize().into_bytes().into()
}

fn hkdf_expand(
    prk: &[u8; PAIRING_SECRET_LEN],
    info: &[u8],
) -> Result<[u8; PAIRING_SECRET_LEN], PairingBindingError> {
    let hkdf = Hkdf::<Sha256>::from_prk(prk).map_err(|_| PairingBindingError::DerivationFailed)?;
    let mut output = [0_u8; PAIRING_SECRET_LEN];
    hkdf.expand(info, &mut output)
        .map_err(|_| PairingBindingError::DerivationFailed)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> PairingBinding {
        PairingBinding::new(
            1,
            [
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
                0x0E, 0x0F,
            ],
            [
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D,
                0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B,
                0x2C, 0x2D, 0x2E, 0x2F,
            ],
            &[
                0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xAB, 0xAC, 0xAD,
                0xAE, 0xAF, 0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xBB,
                0xBC, 0xBD, 0xBE, 0xBF,
            ],
        )
        .unwrap()
    }

    #[test]
    fn matches_published_role_proof_and_media_root_vector() {
        let binding = binding();
        assert_eq!(
            binding.local_proof(PairingRole::AndroidClient).unwrap(),
            [
                0xC2, 0x74, 0xDF, 0xE7, 0x21, 0x35, 0xAE, 0x55, 0x80, 0x34, 0xCA, 0x1E, 0x8A, 0x27,
                0xA4, 0xEA, 0xB7, 0xAA, 0xC9, 0xB8, 0x7A, 0x0C, 0xD9, 0x3C, 0x17, 0xBF, 0x17, 0x12,
                0x6E, 0x27, 0xF4, 0xF2,
            ]
        );
        assert_eq!(
            binding.local_proof(PairingRole::DesktopServer).unwrap(),
            [
                0xF3, 0x47, 0xAB, 0x32, 0x00, 0x6E, 0x8A, 0xCB, 0x22, 0x0F, 0x91, 0xB0, 0xE8, 0x26,
                0xBB, 0xD5, 0x82, 0x57, 0xC5, 0x57, 0x73, 0xD7, 0x28, 0x4F, 0x87, 0x94, 0x21, 0x7F,
                0x89, 0x6B, 0x46, 0x01,
            ]
        );
        assert_eq!(
            binding.media_root().unwrap().as_bytes(),
            &[
                0x25, 0xB4, 0xE8, 0x9F, 0xB6, 0x55, 0xED, 0x5A, 0x9A, 0x9A, 0xD9, 0xB2, 0x8E, 0x6A,
                0xC7, 0xE6, 0xD4, 0x9C, 0x12, 0x46, 0x75, 0x7B, 0x75, 0xF2, 0x0A, 0x12, 0x01, 0x67,
                0xBF, 0xD2, 0x45, 0x2A,
            ]
        );
    }

    #[test]
    fn binding_input_and_role_changes_do_not_verify_the_same_proof() {
        let base = PairingBinding::new(1, [0; 16], [1; 32], &[2; 32]).unwrap();
        let changed_pairing_id = PairingBinding::new(1, [3; 16], [1; 32], &[2; 32]).unwrap();
        let changed_secret = PairingBinding::new(1, [0; 16], [4; 32], &[2; 32]).unwrap();
        let changed_exporter = PairingBinding::new(1, [0; 16], [1; 32], &[5; 32]).unwrap();

        assert_ne!(
            base.local_proof(PairingRole::AndroidClient).unwrap(),
            base.local_proof(PairingRole::DesktopServer).unwrap()
        );
        assert_ne!(
            base.local_proof(PairingRole::AndroidClient).unwrap(),
            changed_pairing_id
                .local_proof(PairingRole::AndroidClient)
                .unwrap()
        );
        assert_ne!(
            base.local_proof(PairingRole::AndroidClient).unwrap(),
            changed_secret
                .local_proof(PairingRole::AndroidClient)
                .unwrap()
        );
        assert_ne!(
            base.local_proof(PairingRole::AndroidClient).unwrap(),
            changed_exporter
                .local_proof(PairingRole::AndroidClient)
                .unwrap()
        );
        let client_proof = base.local_proof(PairingRole::AndroidClient).unwrap();
        assert!(
            base.verify_peer_proof(PairingRole::DesktopServer, &client_proof)
                .unwrap()
        );
        assert!(
            !base
                .verify_peer_proof(PairingRole::AndroidClient, &client_proof)
                .unwrap()
        );
        assert!(
            !base
                .verify_peer_proof(PairingRole::DesktopServer, &client_proof[..31])
                .unwrap()
        );
        assert!(
            !base
                .verify_peer_proof(PairingRole::DesktopServer, &[0; 33])
                .unwrap()
        );
    }

    #[test]
    fn rejects_noncanonical_exporter_lengths() {
        assert!(matches!(
            PairingBinding::new(0, [0; 16], [1; 32], &[2; 32]),
            Err(PairingBindingError::InvalidProtocolVersion)
        ));
        assert!(matches!(
            PairingBinding::new(1, [0; 16], [1; 32], &[2; 31]),
            Err(PairingBindingError::InvalidExporterLength)
        ));
        assert!(matches!(
            PairingBinding::new(1, [0; 16], [1; 32], &[2; 33]),
            Err(PairingBindingError::InvalidExporterLength)
        ));
    }
}
