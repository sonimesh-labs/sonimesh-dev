package com.sonimesh.tlsproviderspike.security

import androidx.test.platform.app.InstrumentationRegistry
import com.sonimesh.sender.pairing.PairingClient
import com.sonimesh.sender.protocol.PairingQr
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

class PairingClientInteropTest {
    @Test
    fun pairs_with_desktop_invitation_listener() {
        val connection = PairingClient.connect(invitation())
        val root = connection.mediaRoot
        try {
            assertEquals("TLSv1.3", connection.socket.session.protocol)
            assertEquals(0, connection.socket.soTimeout)
            assertEquals(32, root.size)
            assertFalse(root.all { it == 0.toByte() })
        } finally {
            connection.close()
        }
        assertTrue(root.all { it == 0.toByte() })
    }

    @Test
    fun rejects_a_changed_desktop_pin() {
        val payload = invitation()
        payload[payload.lastIndex] = (payload.last().toInt() xor 1).toByte()
        assertThrows(javax.net.ssl.SSLException::class.java) { PairingClient.connect(payload) }
    }

    private fun invitation(): ByteArray {
        val args = InstrumentationRegistry.getArguments()
        assumeTrue("pairing lab requires spkiHex", args.containsKey("pairingSpkiHex"))
        val pinHex = requireNotNull(args.getString("pairingSpkiHex"))
        val pin = pinHex.chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val encoded = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val size = PairingQr.encode(
            1, ByteArray(16) { 0x71 }, ByteArray(32) { 0x42 },
            "127.0.0.1", 47610, pin, 120_100, encoded,
        )
        return encoded.copyOf(size)
    }
}
