//! Allocation-free parsing for the SoniMesh media data plane.
//!
//! Cryptography intentionally lives outside this crate. This crate validates the
//! bounded, unauthenticated envelope and returns the exact header bytes that an
//! authenticated ingress implementation must use as AEAD associated data.

mod media_header;
mod replay;

pub use media_header::{
    MAX_ENCRYPTED_PAYLOAD_LEN, MAX_HEADER_LEN, MAX_MEDIA_DATAGRAM_LEN, MEDIA_HEADER_LEN,
    MEDIA_MAGIC, MEDIA_VERSION, MediaHeader, MediaPacket, PacketParseError, parse_media_packet,
};
pub use replay::{ReplayDecision, ReplayWindow};
