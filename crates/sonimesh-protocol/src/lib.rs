//! Allocation-free parsing for the SoniMesh media data plane.
//!
//! Cryptography intentionally lives outside this crate. This crate validates the
//! bounded, unauthenticated envelope and returns the exact header bytes that an
//! authenticated ingress implementation must use as AEAD associated data.

mod control;
mod media_crypto;
mod media_header;
mod media_key;
mod negotiation;
mod pairing_binding;
mod pairing_qr;
mod replay;

/// Generated internal types for the control-plane source schema.
#[allow(missing_docs, clippy::all, clippy::pedantic)]
pub mod generated {
    #![allow(missing_docs, clippy::all, clippy::pedantic)]

    include!(concat!(env!("OUT_DIR"), "/sonimesh.control.v1alpha1.rs"));
}

pub use control::{
    CONTROL_PROTOCOL_VERSION, ControlFrameError, MAX_CONTROL_CAPABILITIES, MAX_CONTROL_CHANNELS,
    MAX_CONTROL_DETAIL_BYTES, MAX_CONTROL_DISPLAY_NAME_BYTES, MAX_CONTROL_FRAME_BYTES,
    MAX_CONTROL_MEDIA_KEY_EPOCH, MAX_CONTROL_SAMPLE_RATE_HZ, MAX_CONTROL_SUPPORTED_VERSIONS,
    MIN_CONTROL_SAMPLE_RATE_HZ, decode_control_frame, encode_control_frame,
    validate_control_envelope,
};
pub use generated::*;
pub use media_crypto::{MAX_MEDIA_PLAINTEXT_LEN, MediaOpener, MediaSealer, ProtectedMediaPacket};
pub use media_header::{
    MAX_ENCRYPTED_PAYLOAD_LEN, MAX_HEADER_LEN, MAX_MEDIA_DATAGRAM_LEN, MEDIA_HEADER_LEN,
    MEDIA_MAGIC, MEDIA_VERSION, MediaHeader, MediaPacket, PacketParseError, parse_media_packet,
};
pub use media_key::{
    MediaCryptoError, MediaDirection, MediaKeyContext, MediaSequence, SessionSecret, media_nonce,
};
pub use negotiation::{ControlNegotiationError, NegotiatedControl, negotiate_hello};
pub use pairing_binding::{
    PAIRING_TLS_EXPORTER_LABEL, PAIRING_TLS_EXPORTER_LEN, PairingBinding, PairingBindingError,
    PairingRole,
};
pub use pairing_qr::{
    MAX_QR_PAYLOAD_LEN, PairingQrPayload, PairingQrView, QR_HOST_MAX_LEN, QR_PAYLOAD_MAGIC,
    QR_PAYLOAD_VERSION, QR_PREFIX_LEN, QR_SPKI_MAX_LEN, QR_SPKI_MIN_LEN, QrPayloadError,
    parse_pairing_qr_payload,
};
pub use replay::{ReplayDecision, ReplayWindow};
