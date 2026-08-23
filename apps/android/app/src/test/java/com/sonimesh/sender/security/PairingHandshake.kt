package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.PairingProofFrame
import com.sonimesh.sender.protocol.PairingQr
import java.security.MessageDigest

/**
 * Unit-test model of the Android-side version-one pairing handshake.
 *
 * This is a host-JVM mirror of the pure desktop composition in Rust: decode a
 * bounded scanned invitation, reject an unexpected invitation before any key
 * derivation, derive the Android-client proof from one TLS exporter
 * transcript, verify the peer desktop-server proof carried in a bounded ADR
 * 0019 wire frame, and release the matching one-time media root only after
 * that proof succeeds.
 *
 * The frame path mirrors the Rust boundary: a received frame must carry this
 * invitation's pairing ID and the DesktopServer role byte before any HKDF
 * verification work runs, so a foreign invitation or a reflected client-role
 * frame is rejected at the wire boundary.
 *
 * It deliberately performs no TLS connection, network I/O, image rendering or
 * scanning, attempt-rate limiting, persistent identity storage, revocation,
 * capture, or media networking. It does not enable pairing and makes no claim
 * about production Android code placement.
 *
 * @throws IllegalArgumentException when the TLS exporter length, scanned QR
 *   payload, protocol version, expected pairing identity, or proof-frame
 *   layout is invalid.
 */
class PairingHandshake private constructor(
    private val scannedInvitation: PairingQr,
    private val binding: PairingTlsBinding,
) {
    private var desktopProofVerified = false

    /** The decoded pairing identity that must match the displayed invitation. */
    val pairingId: ByteArray = scannedInvitation.pairingId.copyOf()

    /**
     * Encodes the exact client-role proof frame to send over the pinned TLS
     * channel into [out] and returns the written byte count. The frame is
     * bound to this invitation's identity and this transcript's proof.
     */
    fun localFrame(out: ByteArray): Int = PairingProofFrame.encode(
        protocolVersion = scannedInvitation.protocolVersion,
        role = PairingProofFrame.ROLE_ANDROID_CLIENT,
        pairingId = scannedInvitation.pairingId,
        proof = binding.localProof(PairingRole.ANDROID_CLIENT),
        out = out,
    )

    /** Derives the proof this endpoint sends over the pinned TLS channel. */
    fun localProof(): ByteArray = binding.localProof(PairingRole.ANDROID_CLIENT)

    /**
     * Verifies a raw ADR 0019 wire frame from the desktop in constant time.
     * The frame must decode canonically, carry this invitation's pairing ID,
     * and present the DesktopServer role byte; any other identity is rejected
     * before derivation. A returned failure leaves the media root unavailable.
     */
    fun verifyDesktopFrame(frame: ByteArray): Boolean {
        val decoded = try {
            PairingProofFrame.decode(frame)
        } catch (_: IllegalArgumentException) {
            return false
        }
        if (!MessageDigest.isEqual(decoded.pairingId, scannedInvitation.pairingId)) {
            return false
        }
        if (decoded.role != PairingProofFrame.ROLE_DESKTOP_SERVER) {
            return false
        }
        val verified =
            binding.verifyPeerProof(PairingRole.ANDROID_CLIENT, decoded.proof)
        desktopProofVerified = verified
        return verified
    }

    /**
     * Verifies the desktop's server-role proof in constant time without a
     * frame envelope. Prefer [verifyDesktopFrame]; this bare-proof entry
     * point remains for callers that already parsed the frame themselves.
     * A returned failure leaves the media root unavailable.
     */
    fun verifyDesktopProof(proof: ByteArray): Boolean {
        val verified = binding.verifyPeerProof(PairingRole.ANDROID_CLIENT, proof)
        desktopProofVerified = verified
        return verified
    }

    /**
     * Returns the transcript-bound media root only after a successful desktop
     * proof.
     *
     * @throws IllegalStateException when the desktop proof has not succeeded.
     */
    fun verifiedMediaRoot(): ByteArray {
        check(desktopProofVerified) { "pairing media root requires a verified desktop proof" }
        return binding.mediaRoot()
    }

    companion object {
        const val TLS_EXPORTER_BYTES = 32
        const val PAIRING_ID_BYTES = 16

        /** Begins the Android role from untrusted scanned bytes. */
        fun beginFromScannedPayload(
            scannedPayload: ByteArray,
            expectedPairingId: ByteArray,
            tlsExporter: ByteArray,
        ): PairingHandshake {
            require(expectedPairingId.size == PAIRING_ID_BYTES) {
                "expected pairing id length is invalid"
            }
            require(tlsExporter.size == TLS_EXPORTER_BYTES) {
                "pairing TLS exporter length is invalid"
            }

            // Decode and bind the invitation before deriving anything. The QR
            // codec enforces every wire bound and canonical field constraint.
            val invitation = PairingQr.decode(scannedPayload)
            require(invitation.protocolVersion == 1) {
                "unsupported pairing protocol version ${invitation.protocolVersion}"
            }
            require(MessageDigest.isEqual(invitation.pairingId, expectedPairingId)) {
                "scanned invitation does not match the displayed pairing id"
            }

            val binding = PairingTlsBinding(
                invitation.protocolVersion,
                invitation.pairingId,
                invitation.pairingSecret,
                tlsExporter,
            )
            return PairingHandshake(invitation, binding)
        }
    }
}
