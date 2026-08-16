package com.sonimesh.sender.security

import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Role bound into a version-one pairing proof. */
enum class PairingRole(val wireValue: Byte) {
    /** Android establishes the TLS connection and proves the client role. */
    ANDROID_CLIENT(0),
    /** Desktop accepts the TLS connection and proves the server role. */
    DESKTOP_SERVER(1);

    /** Returns the opposite role for peer-proof verification. */
    fun peer(): PairingRole = when (this) {
        ANDROID_CLIENT -> DESKTOP_SERVER
        DESKTOP_SERVER -> ANDROID_CLIENT
    }
}

/**
 * Host-JVM mirror of `sonimesh-protocol`'s `PairingBinding`, shared by the
 * pairing vector test so the Android sender derives the same role-separated
 * proofs and one-time media root from the TLS exporter transcript.
 *
 * @throws IllegalArgumentException when the protocol version is unknown or the
 *   pairing ID, pairing secret, or TLS exporter does not have its exact fixed
 *   length, mirroring Rust `PairingBindingError`.
 */
class PairingTlsBinding(
    protocolVersion: Int,
    pairingId: ByteArray,
    pairingSecret: ByteArray,
    tlsExporter: ByteArray,
) {
    private val protocolVersion = protocolVersion.toByte()
    private val pairingId = pairingId.copyOf()
    private val prk: ByteArray

    init {
        require(protocolVersion == CURRENT_PROTOCOL_VERSION) {
            "pairing binding protocol version is invalid"
        }
        require(pairingId.size == PAIRING_ID_BYTES) {
            "pairing binding pairing id length is invalid"
        }
        require(pairingSecret.size == PAIRING_SECRET_BYTES) {
            "pairing binding pairing secret length is invalid"
        }
        require(tlsExporter.size == TLS_EXPORTER_BYTES) {
            "pairing TLS exporter length is invalid"
        }
        prk = hmac(pairingSecret, tlsExporter)
    }

    /** Derives the proof this endpoint sends for its local role. */
    fun localProof(localRole: PairingRole): ByteArray = proofForRole(localRole)

    /**
     * Verifies a peer proof for this endpoint's local role in constant time
     * after rejecting non-canonical proof lengths. A reflected local proof
     * therefore cannot verify as a peer proof.
     */
    fun verifyPeerProof(localRole: PairingRole, proof: ByteArray): Boolean {
        if (proof.size != OUTPUT_BYTES) {
            return false
        }

        val expected = proofForRole(localRole.peer())
        var difference = 0
        for (index in expected.indices) {
            difference = difference or (expected[index].toInt() xor proof[index].toInt())
        }
        expected.fill(0)
        return difference == 0
    }

    /** Derives the one-time media root bound to this pairing. */
    fun mediaRoot(): ByteArray = expand(MEDIA_ROOT_LABEL + byteArrayOf(protocolVersion) + pairingId)

    private fun proofForRole(role: PairingRole): ByteArray = expand(
        PROOF_LABEL + byteArrayOf(protocolVersion, role.wireValue) + pairingId,
    )

    private fun expand(info: ByteArray): ByteArray {
        val output = ByteArray(OUTPUT_BYTES)
        var previous = ByteArray(0)
        var offset = 0
        var counter = 1
        while (offset < output.size) {
            previous = hmac(prk, previous + info + byteArrayOf(counter.toByte()))
            val copied = minOf(previous.size, output.size - offset)
            previous.copyInto(output, offset, 0, copied)
            offset += copied
            counter += 1
        }
        return output
    }

    private fun hmac(key: ByteArray, input: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(input)
    }

    private companion object {
        const val CURRENT_PROTOCOL_VERSION = 1
        const val PAIRING_ID_BYTES = 16
        const val PAIRING_SECRET_BYTES = 32
        const val TLS_EXPORTER_BYTES = 32
        const val OUTPUT_BYTES = 32
        val PROOF_LABEL = "sonimesh pairing proof v1".encodeToByteArray()
        val MEDIA_ROOT_LABEL = "sonimesh pairing media root v1".encodeToByteArray()
    }
}
