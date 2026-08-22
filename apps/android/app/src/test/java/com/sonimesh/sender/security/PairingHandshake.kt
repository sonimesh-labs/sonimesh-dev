package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.PairingQr
import java.security.MessageDigest

/**
 * Unit-test model of the Android-side version-one pairing handshake.
 *
 * This is a host-JVM mirror of the pure desktop composition in Rust: decode a
 * bounded scanned invitation, reject an unexpected invitation before any key
 * derivation, derive the Android-client proof from one TLS exporter
 * transcript, verify the peer desktop-server proof, and release the matching
 * one-time media root only after that proof succeeds.
 *
 * It deliberately performs no TLS connection, network I/O, image rendering or
 * scanning, attempt-rate limiting, persistent identity storage, revocation,
 * capture, or media networking. It does not enable pairing and makes no claim
 * about production Android code placement.
 *
 * @throws IllegalArgumentException when the TLS exporter length, scanned QR
 *   payload, protocol version, or expected pairing identity is invalid.
 */
class PairingHandshake private constructor(
    private val scannedInvitation: PairingQr,
    private val binding: PairingTlsBinding,
) {
    private var desktopProofVerified = false

    /** The decoded pairing identity that must match the displayed invitation. */
    val pairingId: ByteArray = scannedInvitation.pairingId.copyOf()

    /** Derives the proof this endpoint sends over the pinned TLS channel. */
    fun localProof(): ByteArray = binding.localProof(PairingRole.ANDROID_CLIENT)

    /**
     * Verifies the desktop's server-role proof in constant time. A returned
     * failure leaves the media root unavailable.
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
