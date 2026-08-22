package com.sonimesh.sender.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM pin of the published pairing proof-exchange frame vector.
 *
 * The golden bytes match the Rust pins in
 * `crates/sonimesh-protocol/src/pairing_proof.rs` and the Python verifier
 * `tools/check_pairing_proof_frame.py`, so the Android implementation cannot
 * drift from the cross-language first-pairing proof boundary. Encoding and
 * decoding run through the `PairingProofFrame` mirror.
 */
class PairingProofFrameVectorTest {
    private val vectorPairingId = hex("000102030405060708090a0b0c0d0e0f")
    private val vectorClientProof =
        hex("c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2")
    private val vectorServerProof =
        hex("f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601")

    @Test
    fun matches_published_pairing_proof_frame_vector() {
        val encoded = ByteArray(PairingProofFrame.FRAME_BYTES)
        val written = PairingProofFrame.encode(
            protocolVersion = 1,
            role = PairingProofFrame.ROLE_ANDROID_CLIENT,
            pairingId = vectorPairingId,
            proof = vectorClientProof,
            out = encoded,
        )

        assertEquals(ENCODED_VECTOR_HEX.length / 2, written)
        assertEquals(ENCODED_VECTOR_HEX, encoded.toHexString())
    }

    @Test
    fun decode_round_trips_the_published_vector() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        val frame = PairingProofFrame.decode(encoded)

        assertEquals(1, frame.protocolVersion)
        assertEquals(PairingProofFrame.ROLE_ANDROID_CLIENT, frame.role)
        assertTrue(frame.pairingId.contentEquals(vectorPairingId))
        assertTrue(frame.proof.contentEquals(vectorClientProof))

        val reencoded = ByteArray(PairingProofFrame.FRAME_BYTES)
        val written = PairingProofFrame.encode(
            protocolVersion = frame.protocolVersion,
            role = frame.role,
            pairingId = frame.pairingId,
            proof = frame.proof,
            out = reencoded,
        )
        assertEquals(encoded.size, written)
        assertTrue(reencoded.contentEquals(encoded))
    }

    @Test
    fun encodes_the_server_role_vector() {
        val encoded = ByteArray(PairingProofFrame.FRAME_BYTES)
        val written = PairingProofFrame.encode(
            protocolVersion = 1,
            role = PairingProofFrame.ROLE_DESKTOP_SERVER,
            pairingId = vectorPairingId,
            proof = vectorServerProof,
            out = encoded,
        )
        assertEquals(SERVER_VECTOR_HEX.length / 2, written)
        assertEquals(SERVER_VECTOR_HEX, encoded.toHexString())

        val decoded = PairingProofFrame.decode(encoded)
        assertEquals(PairingProofFrame.ROLE_DESKTOP_SERVER, decoded.role)
        assertTrue(decoded.proof.contentEquals(vectorServerProof))
    }

    @Test
    fun rejects_a_foreign_magic() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[0] = 'X'.code.toByte()
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(encoded)
        }
    }

    @Test
    fun rejects_an_unsupported_frame_version() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[4] = 2
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(encoded)
        }
    }

    @Test
    fun rejects_an_unsupported_protocol_version() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[5] = 2
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(encoded)
        }
    }

    @Test
    fun rejects_an_unknown_role_byte() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[6] = 2
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(encoded)
        }
    }

    @Test
    fun rejects_noncanonical_lengths() {
        val truncated = hex(ENCODED_VECTOR_HEX).copyOf(PairingProofFrame.FRAME_BYTES - 1)
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(truncated)
        }
        val trailing = hex(ENCODED_VECTOR_HEX) + byteArrayOf(0)
        assertThrows(IllegalArgumentException::class.java) {
            PairingProofFrame.decode(trailing)
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_an_unknown_role_on_encode() {
        PairingProofFrame.encode(
            protocolVersion = 1,
            role = 7,
            pairingId = vectorPairingId,
            proof = vectorClientProof,
            out = ByteArray(PairingProofFrame.FRAME_BYTES),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_a_short_proof_on_encode() {
        PairingProofFrame.encode(
            protocolVersion = 1,
            role = PairingProofFrame.ROLE_ANDROID_CLIENT,
            pairingId = vectorPairingId,
            proof = vectorClientProof.copyOf(31),
            out = ByteArray(PairingProofFrame.FRAME_BYTES),
        )
    }

    private fun hex(value: String): ByteArray = ByteArray(value.length / 2) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }

    private fun ByteArray.toHexString(): String = joinToString(separator = "") { byte ->
        (byte.toInt() and 0xFF).toString(16).padStart(2, '0')
    }

    private companion object {
        const val ENCODED_VECTOR_HEX =
            "534d5045010100000102030405060708090a0b0c0d0e0f" +
                "c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2"
        const val SERVER_VECTOR_HEX =
            "534d5045010101000102030405060708090a0b0c0d0e0f" +
                "f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601"
    }
}
