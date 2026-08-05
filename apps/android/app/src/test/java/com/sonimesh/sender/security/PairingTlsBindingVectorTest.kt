package com.sonimesh.sender.security

import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingTlsBindingVectorTest {
    @Test
    fun matches_published_role_proof_and_media_root_vector() {
        val binding = PairingTlsBinding(
            protocolVersion = 1,
            pairingId = hex("000102030405060708090a0b0c0d0e0f"),
            pairingSecret = hex("101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"),
            tlsExporter = hex("a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf"),
        )

        assertTrue(
            binding.localProof(PairingRole.ANDROID_CLIENT).contentEquals(
                hex("c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2"),
            ),
        )
        assertTrue(
            binding.localProof(PairingRole.DESKTOP_SERVER).contentEquals(
                hex("f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601"),
            ),
        )
        assertTrue(
            binding.mediaRoot().contentEquals(
                hex("25b4e89fb655ed5a9a9ad9b28e6ac7e6d49c1246757b75f20a120167bfd2452a"),
            ),
        )
    }

    @Test
    fun binding_input_and_role_changes_do_not_verify_the_same_proof() {
        val binding = PairingTlsBinding(1, ByteArray(16), ByteArray(32) { 1 }, ByteArray(32) { 2 })
        val changedPairingId = PairingTlsBinding(1, ByteArray(16) { 3 }, ByteArray(32) { 1 }, ByteArray(32) { 2 })
        val changedSecret = PairingTlsBinding(1, ByteArray(16), ByteArray(32) { 4 }, ByteArray(32) { 2 })
        val changedExporter = PairingTlsBinding(1, ByteArray(16), ByteArray(32) { 1 }, ByteArray(32) { 5 })

        assertFalse(binding.localProof(PairingRole.ANDROID_CLIENT).contentEquals(binding.localProof(PairingRole.DESKTOP_SERVER)))
        assertFalse(binding.localProof(PairingRole.ANDROID_CLIENT).contentEquals(changedPairingId.localProof(PairingRole.ANDROID_CLIENT)))
        assertFalse(binding.localProof(PairingRole.ANDROID_CLIENT).contentEquals(changedSecret.localProof(PairingRole.ANDROID_CLIENT)))
        assertFalse(binding.localProof(PairingRole.ANDROID_CLIENT).contentEquals(changedExporter.localProof(PairingRole.ANDROID_CLIENT)))

        val clientProof = binding.localProof(PairingRole.ANDROID_CLIENT)
        assertTrue(binding.verifyPeerProof(PairingRole.DESKTOP_SERVER, clientProof))
        assertFalse(binding.verifyPeerProof(PairingRole.ANDROID_CLIENT, clientProof))
        assertFalse(binding.verifyPeerProof(PairingRole.DESKTOP_SERVER, clientProof.copyOf(31)))
        assertFalse(binding.verifyPeerProof(PairingRole.DESKTOP_SERVER, clientProof + byteArrayOf(0)))
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_a_noncanonical_exporter_length() {
        PairingTlsBinding(1, ByteArray(16), ByteArray(32), ByteArray(31))
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_an_unknown_protocol_version() {
        PairingTlsBinding(2, ByteArray(16), ByteArray(32), ByteArray(32))
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_an_oversized_exporter() {
        PairingTlsBinding(1, ByteArray(16), ByteArray(32), ByteArray(33))
    }

    private fun hex(value: String): ByteArray = ByteArray(value.length / 2) { index ->
        value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
    }
}

private enum class PairingRole(val wireValue: Byte) {
    ANDROID_CLIENT(0),
    DESKTOP_SERVER(1);

    fun peer(): PairingRole = when (this) {
        ANDROID_CLIENT -> DESKTOP_SERVER
        DESKTOP_SERVER -> ANDROID_CLIENT
    }
}

private class PairingTlsBinding(
    protocolVersion: Int,
    pairingId: ByteArray,
    pairingSecret: ByteArray,
    tlsExporter: ByteArray,
) {
    private val protocolVersion = protocolVersion.toByte()
    private val pairingId = pairingId.copyOf()
    private val prk: ByteArray

    init {
        require(protocolVersion == CURRENT_PROTOCOL_VERSION)
        require(pairingId.size == PAIRING_ID_BYTES)
        require(pairingSecret.size == PAIRING_SECRET_BYTES)
        require(tlsExporter.size == TLS_EXPORTER_BYTES)
        prk = hmac(pairingSecret, tlsExporter)
    }

    fun localProof(localRole: PairingRole): ByteArray = proofForRole(localRole)

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
