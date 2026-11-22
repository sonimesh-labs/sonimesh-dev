package com.sonimesh.sender.pairing

import com.sonimesh.sender.protocol.PairingProofFrame
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingDerivationTest {
    @Test
    fun matches_published_proofs_and_media_root() {
        val id = ByteArray(16) { it.toByte() }
        val secret = ByteArray(32) { (it + 0x10).toByte() }
        val exporter = ByteArray(32) { (it + 0xa0).toByte() }
        val prk = pairingHmac(secret, exporter)
        val proofLabel = "sonimesh pairing proof v1".encodeToByteArray()
        val rootLabel = "sonimesh pairing media root v1".encodeToByteArray()
        assertArrayEquals(hex("c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2"), pairingDerive(prk, proofLabel, id, 0))
        assertArrayEquals(hex("f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601"), pairingDerive(prk, proofLabel, id, 1))
        assertArrayEquals(hex("25b4e89fb655ed5a9a9ad9b28e6ac7e6d49c1246757b75f20a120167bfd2452a"), pairingDerive(prk, rootLabel, id, null))
    }

    @Test
    fun only_accepts_the_expected_desktop_frame() {
        val id = ByteArray(16) { it.toByte() }
        val proof = ByteArray(32) { (it + 1).toByte() }
        val wire = ByteArray(PairingProofFrame.FRAME_BYTES)
        PairingProofFrame.encode(1, PairingProofFrame.ROLE_DESKTOP_SERVER, id, proof, wire)
        assertTrue(verifyDesktopFrame(wire, id, proof))
        assertFalse(verifyDesktopFrame(wire.copyOf(wire.size - 1), id, proof))
        assertFalse(verifyDesktopFrame(wire, ByteArray(16), proof))
        assertFalse(verifyDesktopFrame(wire, id, ByteArray(32)))
        wire[6] = PairingProofFrame.ROLE_ANDROID_CLIENT.toByte()
        assertFalse(verifyDesktopFrame(wire, id, proof))
    }

    private fun hex(value: String): ByteArray = value.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
}
