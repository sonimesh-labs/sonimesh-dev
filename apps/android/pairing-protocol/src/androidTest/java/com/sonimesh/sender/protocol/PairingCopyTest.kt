package com.sonimesh.sender.protocol

import org.junit.Assert.assertArrayEquals
import org.junit.Test

class PairingCopyTest {
    @Test
    fun decoded_qr_fields_are_independent_of_the_input() {
        val secret = ByteArray(32) { it.toByte() }
        val spki = ByteArray(32) { (it + 32).toByte() }
        val encoded = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val size = PairingQr.encode(1, ByteArray(16), secret, "desktop", 47610, spki, 1, encoded)
        val payload = encoded.copyOf(size)
        val decoded = PairingQr.decode(payload)
        payload.fill(0)
        assertArrayEquals(secret, decoded.pairingSecret)
        assertArrayEquals(spki, decoded.desktopTlsSpki)
    }
}
