package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.PairingQr
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the Android-side handshake composition to the same published vectors
 * and role rules used by the Rust desktop composition. This test runs on the
 * host JVM only; it does not exercise TLS, a device camera, pairing storage,
 * or the capture service.
 */
class PairingHandshakeTest {
    private val invitationId = hex("000102030405060708090a0b0c0d0e0f")
    private val tlsExporter = hex("a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf")
    private val scannedInvitation = hex(
        "534d5152010114b9fa0030000102030405060708090a0b0c0d0e0f" +
            "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f" +
            "00000000001e84806465736b746f702d30312e6c61622e6c6f63616c" +
            "404142434445464748494a4b4c4d4e4f50515253545556575859" +
            "5a5b5c5d5e5f606162636465666768696a6b6c6d6e6f",
    )
    private val expectedClientProof =
        hex("c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2")
    private val expectedDesktopProof =
        hex("f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601")

    @Test
    fun trusted_handshake_derives_the_published_client_proof_and_media_root() {
        val handshake = beginHandshake()

        assertArrayEquals(expectedClientProof, handshake.localProof())
        assertTrue(handshake.verifyDesktopProof(expectedDesktopProof))

        val mediaRoot = handshake.verifiedMediaRoot()
        assertEquals(
            "25b4e89fb655ed5a9a9ad9b28e6ac7e6d49c1246757b75f20a120167bfd2452a",
            mediaRoot.toHexString(),
        )
    }

    @Test
    fun reflected_client_proof_is_rejected_and_the_media_root_stays_locked() {
        val handshake = beginHandshake()

        assertFalse(handshake.verifyDesktopProof(handshake.localProof()))
        assertFalse(handshake.verifyDesktopProof(expectedClientProof))
        assertThrows(IllegalStateException::class.java) { handshake.verifiedMediaRoot() }
    }

    @Test
    fun wrong_desktop_proof_does_not_unlock_the_media_root() {
        val handshake = beginHandshake()
        val alteredProof = expectedDesktopProof.copyOf().also {
            it[0] = (it[0].toInt() xor 0x01).toByte()
        }

        assertFalse(handshake.verifyDesktopProof(alteredProof))
        assertFalse(handshake.verifyDesktopProof(alteredProof + byteArrayOf(0)))
        assertThrows(IllegalStateException::class.java) { handshake.verifiedMediaRoot() }
    }

    @Test
    fun foreign_pairing_id_fails_before_local_proof_derivation() {
        val foreignId = ByteArray(16) { 0x77 }
        assertThrows(IllegalArgumentException::class.java) {
            PairingHandshake.beginFromScannedPayload(scannedInvitation, foreignId, tlsExporter)
        }
    }

    @Test
    fun corrupted_scan_fails_before_local_proof_derivation() {
        val corrupted = scannedInvitation.copyOf().also {
            it[6] = (it[6].toInt() + 1).toByte()
        }

        assertThrows(IllegalArgumentException::class.java) {
            beginHandshake(corrupted)
        }
    }

    @Test
    fun noncanonical_exporter_lengths_fail_before_decoding() {
        listOf(0, 31, 33, 64).forEach { length ->
            assertThrows(IllegalArgumentException::class.java) {
                PairingHandshake.beginFromScannedPayload(
                    scannedInvitation,
                    invitationId,
                    ByteArray(length),
                )
            }
        }
    }

    @Test
    fun decoded_invitation_identity_matches_the_expected_bytes() {
        assertEquals(PairingQr.PAYLOAD_VERSION, 1)
        assertArrayEquals(invitationId, beginHandshake().pairingId)
    }

    @Test
    fun different_exporter_transcript_produces_a_different_client_proof() {
        val changedExporter = tlsExporter.copyOf().also {
            it[31] = (it[31].toInt() xor 0x01).toByte()
        }
        val changed = PairingHandshake.beginFromScannedPayload(
            scannedInvitation,
            invitationId,
            changedExporter,
        )

        assertFalse(changed.localProof().contentEquals(beginHandshake().localProof()))
        assertFalse(changed.verifyDesktopProof(expectedDesktopProof))
    }

    private fun beginHandshake(payload: ByteArray = scannedInvitation): PairingHandshake =
        PairingHandshake.beginFromScannedPayload(payload, invitationId, tlsExporter)

    private fun hex(value: String): ByteArray = ByteArray(value.length / 2) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }

    private fun ByteArray.toHexString(): String = joinToString(separator = "") { byte ->
        (byte.toInt() and 0xFF).toString(16).padStart(2, '0')
    }
}
