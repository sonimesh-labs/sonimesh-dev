//! Bounded version-one pairing proof-exchange frame encoding and decoding.
//!
//! After the pinned TLS 1.3 handshake completes, each peer sends exactly one
//! 32-byte transcript proof inside that channel (ADR 0008). This module fixes
//! the on-the-wire representation of that exchange as a fixed-length frame so
//! two independent implementations cannot disagree about the bytes around the
//! proofs. Encoding and decoding are allocation-free and fail closed: the
//! frame length is exact, every declared field is validated before use, and
//! no field is optional or variable-length. This module performs no TLS,
//! derivation, verification, pairing-record consumption, or trust storage;
//! the platform pairing owner wraps [`crate::PairingBinding`] outputs in this
//! frame and feeds a decoded frame back into the same pure binding.

use core::fmt;

use crate::{CONTROL_PROTOCOL_VERSION, pairing_binding::PairingRole};

/// Frame-family marker for the version-one pairing proof frame (`SMPE`).
pub const PROOF_FRAME_MAGIC: [u8; 4] = *b"SMPE";
/// Current pairing proof frame version.
pub const PROOF_FRAME_VERSION: u8 = 1;
/// Exact encoded length of a complete pairing proof frame.
///
/// Magic, frame version, control protocol version, role byte, the 128-bit
/// pairing ID, and one 256-bit proof.
pub const PROOF_FRAME_LEN: usize = 4 + 1 + 1 + 1 + 16 + 32;

const FRAME_VERSION_OFFSET: usize = 4;
const FRAME_PROTOCOL_OFFSET: usize = 5;
const FRAME_ROLE_OFFSET: usize = 6;
const FRAME_PAIRING_ID_OFFSET: usize = 7;
const FRAME_PROOF_OFFSET: usize = FRAME_PAIRING_ID_OFFSET + 16;

/// Failure returned while encoding or decoding a pairing proof frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingProofFrameError {
    /// Frame cannot contain the complete fixed layout.
    Truncated,
    /// Frame marker belongs to another protocol.
    InvalidMagic,
    /// Frame version is unsupported.
    UnsupportedFrameVersion(u8),
    /// Control protocol version is not the supported version one.
    InvalidProtocolVersion(u8),
    /// Role byte is neither Android-client nor Desktop-server.
    InvalidRole(u8),
}

impl fmt::Display for PairingProofFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => formatter.write_str("pairing proof frame is truncated"),
            Self::InvalidMagic => {
                formatter.write_str("pairing proof frame has an invalid magic value")
            }
            Self::UnsupportedFrameVersion(version) => {
                write!(
                    formatter,
                    "unsupported pairing proof frame version {version}"
                )
            }
            Self::InvalidProtocolVersion(version) => {
                write!(formatter, "unsupported pairing protocol version {version}")
            }
            Self::InvalidRole(role) => write!(formatter, "invalid pairing role byte {role}"),
        }
    }
}

impl std::error::Error for PairingProofFrameError {}

/// One direction of the version-one pairing proof exchange.
///
/// All fields are fixed-width, so the same owned type serves the sender-side
/// [`Self::encode_into`] and the receiver-side
/// [`parse_pairing_proof_frame`]. The proof is a derived transcript output;
/// its `Debug` output stays redacted like every other pairing material.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PairingProofFrame {
    /// Control protocol version the proofs were derived under.
    pub protocol_version: u8,
    /// Role whose proof this frame carries.
    pub role: PairingRole,
    /// One-time 128-bit pairing identifier bound into the proof.
    pub pairing_id: [u8; 16],
    /// The 256-bit transcript proof carried for `role`.
    pub proof: [u8; 32],
}

impl PairingProofFrame {
    /// Validates the frame fields against the version-one bounds.
    fn validate(&self) -> Result<(), PairingProofFrameError> {
        if u32::from(self.protocol_version) != CONTROL_PROTOCOL_VERSION {
            return Err(PairingProofFrameError::InvalidProtocolVersion(
                self.protocol_version,
            ));
        }
        Ok(())
    }

    /// Returns the exact encoded byte length.
    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        PROOF_FRAME_LEN
    }

    /// Encodes the frame into `out` and returns the written length.
    ///
    /// # Errors
    ///
    /// Returns [`PairingProofFrameError`] when a field is outside the
    /// version-one bounds, or when `out` is shorter than
    /// [`PROOF_FRAME_LEN`].
    pub fn encode_into(&self, out: &mut [u8]) -> Result<usize, PairingProofFrameError> {
        self.validate()?;
        let out = out
            .get_mut(..PROOF_FRAME_LEN)
            .ok_or(PairingProofFrameError::Truncated)?;

        out[0..4].copy_from_slice(&PROOF_FRAME_MAGIC);
        out[FRAME_VERSION_OFFSET] = PROOF_FRAME_VERSION;
        out[FRAME_PROTOCOL_OFFSET] = self.protocol_version;
        out[FRAME_ROLE_OFFSET] = self.role as u8;
        out[FRAME_PAIRING_ID_OFFSET..FRAME_PAIRING_ID_OFFSET + 16]
            .copy_from_slice(&self.pairing_id);
        out[FRAME_PROOF_OFFSET..FRAME_PROOF_OFFSET + 32].copy_from_slice(&self.proof);
        Ok(PROOF_FRAME_LEN)
    }
}

impl fmt::Debug for PairingProofFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingProofFrame")
            .field("protocol_version", &self.protocol_version)
            .field("role", &self.role)
            .field("pairing_id", &"[REDACTED]")
            .field("proof", &"[REDACTED]")
            .finish()
    }
}

/// Parses an untrusted pairing proof frame without allocation.
///
/// # Errors
///
/// Returns [`PairingProofFrameError`] before any data is copied when the
/// frame has any length other than the exact fixed length, carries a foreign
/// magic, an unsupported frame version, a non-version-one control protocol
/// version, or a role byte outside the two defined roles.
pub fn parse_pairing_proof_frame(
    frame: &[u8],
) -> Result<PairingProofFrame, PairingProofFrameError> {
    if frame.len() < PROOF_FRAME_LEN {
        return Err(PairingProofFrameError::Truncated);
    }
    if frame.len() > PROOF_FRAME_LEN {
        return Err(PairingProofFrameError::Truncated);
    }
    if frame[0..4] != PROOF_FRAME_MAGIC {
        return Err(PairingProofFrameError::InvalidMagic);
    }
    let frame_version = frame[FRAME_VERSION_OFFSET];
    if frame_version != PROOF_FRAME_VERSION {
        return Err(PairingProofFrameError::UnsupportedFrameVersion(
            frame_version,
        ));
    }
    let protocol_version = frame[FRAME_PROTOCOL_OFFSET];
    if u32::from(protocol_version) != CONTROL_PROTOCOL_VERSION {
        return Err(PairingProofFrameError::InvalidProtocolVersion(
            protocol_version,
        ));
    }
    let role_byte = frame[FRAME_ROLE_OFFSET];
    let role = match role_byte {
        0 => PairingRole::AndroidClient,
        1 => PairingRole::DesktopServer,
        _ => return Err(PairingProofFrameError::InvalidRole(role_byte)),
    };

    let mut pairing_id = [0_u8; 16];
    pairing_id.copy_from_slice(
        frame
            .get(FRAME_PAIRING_ID_OFFSET..FRAME_PAIRING_ID_OFFSET + 16)
            .ok_or(PairingProofFrameError::Truncated)?,
    );
    let mut proof = [0_u8; 32];
    proof.copy_from_slice(
        frame
            .get(FRAME_PROOF_OFFSET..FRAME_PROOF_OFFSET + 32)
            .ok_or(PairingProofFrameError::Truncated)?,
    );

    Ok(PairingProofFrame {
        protocol_version,
        role,
        pairing_id,
        proof,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PairingBinding, pairing_binding::PAIRING_TLS_EXPORTER_LEN};

    /// The published client proof from the TLS-binding vector, wrapped in the
    /// fixed frame layout.
    const VECTOR_ENCODED_HEX: &str = "534d5045010100000102030405060708090a0b0c0d0e0fc274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2";

    const VECTOR_PAIRING_ID: [u8; 16] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ];
    const VECTOR_CLIENT_PROOF: [u8; 32] = [
        0xC2, 0x74, 0xDF, 0xE7, 0x21, 0x35, 0xAE, 0x55, 0x80, 0x34, 0xCA, 0x1E, 0x8A, 0x27, 0xA4,
        0xEA, 0xB7, 0xAA, 0xC9, 0xB8, 0x7A, 0x0C, 0xD9, 0x3C, 0x17, 0xBF, 0x17, 0x12, 0x6E, 0x27,
        0xF4, 0xF2,
    ];

    fn unhex(value: &str) -> Vec<u8> {
        (0..value.len() / 2)
            .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).unwrap())
            .collect()
    }

    fn vector_frame() -> PairingProofFrame {
        PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: VECTOR_PAIRING_ID,
            proof: VECTOR_CLIENT_PROOF,
        }
    }

    #[test]
    fn frame_length_matches_the_documented_layout() {
        assert_eq!(PROOF_FRAME_LEN, 55);
        assert_eq!(FRAME_PROOF_OFFSET + 32, PROOF_FRAME_LEN);
        assert_eq!(vector_frame().encoded_len(), PROOF_FRAME_LEN);
    }

    #[test]
    fn matches_published_pairing_proof_frame_vector() {
        let expected = unhex(VECTOR_ENCODED_HEX);
        assert_eq!(expected.len(), PROOF_FRAME_LEN);

        let mut encoded = [0_u8; PROOF_FRAME_LEN];
        let written = vector_frame().encode_into(&mut encoded).unwrap();
        assert_eq!(written, expected.len());
        assert_eq!(encoded.as_slice(), expected.as_slice());
    }

    #[test]
    fn decode_round_trips_the_published_vector() {
        let encoded = unhex(VECTOR_ENCODED_HEX);
        let frame = parse_pairing_proof_frame(&encoded).unwrap();
        assert_eq!(frame.protocol_version, 1);
        assert_eq!(frame.role, PairingRole::AndroidClient);
        assert_eq!(frame.pairing_id, VECTOR_PAIRING_ID);
        assert_eq!(frame.proof, VECTOR_CLIENT_PROOF);

        let mut reencoded = [0_u8; PROOF_FRAME_LEN];
        let written = frame.encode_into(&mut reencoded).unwrap();
        assert_eq!(&reencoded[..written], encoded.as_slice());
    }

    #[test]
    fn encode_rejects_a_short_output_buffer() {
        let mut out = [0_u8; PROOF_FRAME_LEN - 1];
        assert_eq!(
            vector_frame().encode_into(&mut out),
            Err(PairingProofFrameError::Truncated)
        );
    }

    #[test]
    fn encode_rejects_an_invalid_protocol_version() {
        let mut frame = vector_frame();
        frame.protocol_version = 2;
        assert_eq!(
            frame.encode_into(&mut [0_u8; PROOF_FRAME_LEN]),
            Err(PairingProofFrameError::InvalidProtocolVersion(2))
        );
    }

    #[test]
    fn decode_rejects_wrong_lengths_and_foreign_magic() {
        let encoded = unhex(VECTOR_ENCODED_HEX);

        let truncated = encoded[..PROOF_FRAME_LEN - 1].to_vec();
        assert_eq!(
            parse_pairing_proof_frame(&truncated),
            Err(PairingProofFrameError::Truncated)
        );
        assert_eq!(
            parse_pairing_proof_frame(&[]),
            Err(PairingProofFrameError::Truncated)
        );
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            parse_pairing_proof_frame(&trailing),
            Err(PairingProofFrameError::Truncated)
        );

        let mut foreign = encoded.clone();
        foreign[0] = b'X';
        assert_eq!(
            parse_pairing_proof_frame(&foreign),
            Err(PairingProofFrameError::InvalidMagic)
        );
    }

    #[test]
    fn decode_rejects_unsupported_versions_and_roles() {
        let encoded = unhex(VECTOR_ENCODED_HEX);

        let mut bad_frame_version = encoded.clone();
        bad_frame_version[FRAME_VERSION_OFFSET] = 2;
        assert_eq!(
            parse_pairing_proof_frame(&bad_frame_version),
            Err(PairingProofFrameError::UnsupportedFrameVersion(2))
        );

        let mut bad_protocol = encoded.clone();
        bad_protocol[FRAME_PROTOCOL_OFFSET] = 9;
        assert_eq!(
            parse_pairing_proof_frame(&bad_protocol),
            Err(PairingProofFrameError::InvalidProtocolVersion(9))
        );

        let mut bad_role = encoded;
        bad_role[FRAME_ROLE_OFFSET] = 2;
        assert_eq!(
            parse_pairing_proof_frame(&bad_role),
            Err(PairingProofFrameError::InvalidRole(2))
        );
    }

    #[test]
    fn server_role_frame_round_trips_with_the_published_server_proof() {
        let frame = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::DesktopServer,
            pairing_id: VECTOR_PAIRING_ID,
            proof: [
                0xF3, 0x47, 0xAB, 0x32, 0x00, 0x6E, 0x8A, 0xCB, 0x22, 0x0F, 0x91, 0xB0, 0xE8, 0x26,
                0xBB, 0xD5, 0x82, 0x57, 0xC5, 0x57, 0x73, 0xD7, 0x28, 0x4F, 0x87, 0x94, 0x21, 0x7F,
                0x89, 0x6B, 0x46, 0x01,
            ],
        };
        let mut encoded = [0_u8; PROOF_FRAME_LEN];
        frame.encode_into(&mut encoded).unwrap();
        let decoded = parse_pairing_proof_frame(&encoded).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(decoded.role, PairingRole::DesktopServer);
    }

    #[test]
    fn exchanged_frames_drive_the_pure_binding_verification() {
        // Both roles derive their proofs from the published TLS-binding
        // inputs; the frames carry them across the boundary unchanged, and a
        // reflected proof still fails closed after a frame round trip.
        let exporter = [0xA5_u8; PAIRING_TLS_EXPORTER_LEN];
        let desktop = PairingBinding::new(1, VECTOR_PAIRING_ID, [0x11; 32], &exporter).unwrap();
        let android = PairingBinding::new(1, VECTOR_PAIRING_ID, [0x11; 32], &exporter).unwrap();

        let client_proof = android.local_proof(PairingRole::AndroidClient).unwrap();
        let client_frame = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: VECTOR_PAIRING_ID,
            proof: client_proof,
        };
        let mut encoded = [0_u8; PROOF_FRAME_LEN];
        client_frame.encode_into(&mut encoded).unwrap();
        let received = parse_pairing_proof_frame(&encoded).unwrap();

        assert!(received.pairing_id == VECTOR_PAIRING_ID);
        assert!(
            desktop
                .verify_peer_proof(PairingRole::DesktopServer, &received.proof)
                .unwrap()
        );
        assert!(
            !desktop
                .verify_peer_proof(PairingRole::AndroidClient, &received.proof)
                .unwrap()
        );

        let server_proof = desktop.local_proof(PairingRole::DesktopServer).unwrap();
        let server_frame = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::DesktopServer,
            pairing_id: VECTOR_PAIRING_ID,
            proof: server_proof,
        };
        let mut encoded = [0_u8; PROOF_FRAME_LEN];
        server_frame.encode_into(&mut encoded).unwrap();
        let received = parse_pairing_proof_frame(&encoded).unwrap();
        assert!(
            android
                .verify_peer_proof(PairingRole::AndroidClient, &received.proof)
                .unwrap()
        );
    }

    #[test]
    fn debug_output_is_redacted() {
        let debug = format!("{:?}", vector_frame());
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("c274"));
        let decoded = parse_pairing_proof_frame(&unhex(VECTOR_ENCODED_HEX)).unwrap();
        let debug = format!("{decoded:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("c274"));
    }

    #[test]
    fn error_display_is_actionable() {
        assert_eq!(
            PairingProofFrameError::Truncated.to_string(),
            "pairing proof frame is truncated"
        );
        assert_eq!(
            PairingProofFrameError::InvalidMagic.to_string(),
            "pairing proof frame has an invalid magic value"
        );
        assert_eq!(
            PairingProofFrameError::UnsupportedFrameVersion(2).to_string(),
            "unsupported pairing proof frame version 2"
        );
        assert_eq!(
            PairingProofFrameError::InvalidProtocolVersion(3).to_string(),
            "unsupported pairing protocol version 3"
        );
        assert_eq!(
            PairingProofFrameError::InvalidRole(7).to_string(),
            "invalid pairing role byte 7"
        );
    }
}
