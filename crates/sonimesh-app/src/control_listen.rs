//! Invitation-gated pinned TLS control listener for the desktop.
//!
//! ADR 0021 names the desktop stack; this module owns the accept shape the
//! production server will run: one bounded listener that accepts a
//! connection, completes the pinned TLS 1.3 handshake, reads exactly one
//! ADR 0019 proof frame from that channel, admits it through
//! [`verify_scanned_proof`] (which charges the attempt, consumes a verified
//! invitation, and renders the exact reply frame), and writes the reply back
//! into the channel. The caller receives either a trusted outcome with the
//! transcript-bound media root and the still-open framed channel, or a typed
//! failure; nothing persists between attempts beyond the invitation
//! lifecycle itself.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use zeroize::Zeroizing;

pub use sonimesh_network::ServerConfig as TlsServerConfig;
use sonimesh_network::{DesktopTlsError, PinnedServerChannel, accept_pinned_server};
use sonimesh_protocol::{PAIRING_TLS_EXPORTER_LEN, PROOF_FRAME_LEN};

use crate::{
    PairingHandshakeError, PairingHandshakeOutcome, PairingId, PairingInvitationController,
    PairingSource, ScannedProofAttempt, SessionState, verify_scanned_proof,
};

/// Failure while accepting and verifying one pinned pairing connection.
#[derive(Debug)]
pub enum ControlListenError {
    /// Binding or accepting on the control socket failed.
    Socket(std::io::Error),
    /// The pinned TLS handshake failed or did not negotiate TLS 1.3.
    Tls(DesktopTlsError),
    /// Reading the peer's proof frame failed before parsing.
    PeerFrameIo(std::io::Error),
    /// The composed handshake rejected the attempt (QR decode, identity,
    /// admission, or proof verification).
    Handshake(crate::PairingHandshakeError),
    /// Writing the reply frame failed after a trusted outcome; the session
    /// is already trusted but this channel is unusable.
    ReplyWrite(std::io::Error),
}

impl core::fmt::Display for ControlListenError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Socket(error) => write!(formatter, "control listener I/O failed: {error}"),
            Self::Tls(error) => write!(formatter, "pinned TLS handshake failed: {error}"),
            Self::PeerFrameIo(error) => {
                write!(formatter, "peer proof frame could not be read: {error}")
            }
            Self::Handshake(error) => {
                write!(formatter, "pairing handshake rejected the attempt: {error}")
            }
            Self::ReplyWrite(error) => {
                write!(formatter, "reply frame could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for ControlListenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tls(error) => Some(error),
            Self::Handshake(error) => Some(error),
            _ => None,
        }
    }
}

/// One trusted desktop pairing outcome over a live pinned channel.
pub struct TrustedControlChannel {
    channel: PinnedServerChannel,
    exporter: Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
}

impl TrustedControlChannel {
    /// Hands the framed TLS channel to the ADR 0020 wrapper.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        PinnedServerChannel,
        Zeroizing<[u8; PAIRING_TLS_EXPORTER_LEN]>,
    ) {
        (self.channel, self.exporter)
    }
}

impl core::fmt::Debug for TrustedControlChannel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("TrustedControlChannel([REDACTED])")
    }
}

/// Everything one invitation needs to accept and verify its peer.
///
/// `scanned_payload` must be the exact bytes that
/// [`begin_from_rendered_payload`](crate::begin_from_rendered_payload)
/// wrote, so the admitted scan is the rendered invitation itself.
pub struct InvitationInputs<'a> {
    /// The one-time 128-bit invitation identifier.
    pub invitation: PairingId,
    /// The one-time 256-bit pairing secret rendered into that invitation.
    pub pairing_secret: [u8; 32],
    /// The exact rendered QR payload bytes.
    pub scanned_payload: &'a [u8],
}

impl core::fmt::Debug for InvitationInputs<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("InvitationInputs")
            .field("invitation", &"[REDACTED]")
            .field("pairing_secret", &"[REDACTED]")
            .field("scanned_payload_len", &self.scanned_payload.len())
            .finish()
    }
}

/// Accepts exactly one pinned pairing connection against the active
/// invitation and returns the trusted channel with its media root.
///
/// The listener must already be bound by the caller so the same endpoint
/// that rendered into the QR payload is used here. The function performs
/// the full sequence: pinned TLS handshake, one bounded proof-frame read,
/// attempt admission through the real owner, reply write.
///
/// # Errors
///
/// Returns [`ControlListenError`] for every failure shape along that
/// sequence. A well-formed but wrong proof surfaces as
/// [`ControlListenError::Handshake`] wrapping
/// [`PairingHandshakeError::ProofRejected`] while the invitation keeps its
/// remaining attempts.
pub fn accept_pairing_connection(
    listener: &TcpListener,
    tls: Arc<TlsServerConfig>,
    pairing: &mut PairingInvitationController<4>,
    session: &mut SessionState,
    now_ms: u64,
    source: PairingSource,
    inputs: &InvitationInputs<'_>,
) -> Result<(TrustedControlChannel, sonimesh_protocol::SessionSecret), ControlListenError> {
    let (stream, _) = listener.accept().map_err(ControlListenError::Socket)?;
    let (mut channel, exporter) = accept_pinned_server(tls, &inputs.invitation.as_bytes(), stream)
        .map_err(ControlListenError::Tls)?;

    // Read exactly one canonical peer frame from the live channel.
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    channel
        .read_exact(&mut wire)
        .map_err(ControlListenError::PeerFrameIo)?;

    let outcome = verify_scanned_proof(
        pairing,
        session,
        now_ms,
        source,
        exporter.as_ref(),
        ScannedProofAttempt {
            scanned_payload: inputs.scanned_payload,
            expected_pairing_id: inputs.invitation,
            pairing_secret: inputs.pairing_secret,
            peer_proof_frame: &wire,
        },
    )
    .map_err(ControlListenError::Handshake)?;

    let PairingHandshakeOutcome::Ready {
        media_root,
        reply_frame,
    } = outcome
    else {
        // verify_scanned_proof returns Ok(Rejected{..}) for a well-formed
        // wrong proof; surface it as the composition-level rejection.
        return Err(ControlListenError::Handshake(
            PairingHandshakeError::ProofRejected,
        ));
    };

    let mut reply_wire = [0_u8; PROOF_FRAME_LEN];
    reply_frame
        .encode_into(&mut reply_wire)
        .map_err(|_| ControlListenError::ReplyWrite(std::io::Error::other("frame overflow")))?;
    channel
        .write_all(&reply_wire)
        .and_then(|()| channel.flush())
        .map_err(ControlListenError::ReplyWrite)?;

    Ok((TrustedControlChannel { channel, exporter }, media_root))
}
