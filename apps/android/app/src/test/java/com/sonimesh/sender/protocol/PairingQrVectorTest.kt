package com.sonimesh.sender.protocol

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * JVM pin of the published pairing QR payload vector.
 *
 * The golden bytes match the Rust pins in
 * `crates/sonimesh-protocol/src/pairing_qr.rs` and the Python verifier
 * `tools/check_pairing_qr_vectors.py`, so the Android implementation cannot
 * drift from the cross-language first-pairing QR boundary. Encoding and
 * decoding run through the `PairingQr` mirror.
 */
class PairingQrVectorTest {
    private val vectorPairingId = hex("000102030405060708090a0b0c0d0e0f")
    private val vectorSecret =
        hex("101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f")
    private val vectorHost = "desktop-01.lab.local"
    private val vectorPort = 47610
    private val vectorSpki = hex(
        "404142434445464748494a4b4c4d4e4f50515253545556575859" +
            "5a5b5c5d5e5f606162636465666768696a6b6c6d6e6f",
    )
    private val vectorExpiryMs = 2_000_000L

    @Test
    fun matches_published_pairing_qr_vector() {
        val encoded = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val written = PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = vectorHost,
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = vectorExpiryMs,
            out = encoded,
        )

        assertEquals(ENCODED_VECTOR_HEX.length / 2, written)
        assertEquals(ENCODED_VECTOR_HEX, encoded.copyOf(written).toHexString())
    }

    @Test
    fun decode_round_trips_the_published_vector() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        val view = PairingQr.decode(encoded)

        assertEquals(1, view.protocolVersion)
        assertTrue(view.pairingId.contentEquals(vectorPairingId))
        assertTrue(view.pairingSecret.contentEquals(vectorSecret))
        assertEquals(vectorHost, view.controlHost)
        assertEquals(vectorPort, view.controlPort)
        assertTrue(view.desktopTlsSpki.contentEquals(vectorSpki))
        assertEquals(vectorExpiryMs, view.expiresAtMs)

        val reencoded = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val written = PairingQr.encode(
            protocolVersion = view.protocolVersion,
            pairingId = view.pairingId,
            pairingSecret = view.pairingSecret,
            controlHost = view.controlHost,
            controlPort = view.controlPort,
            desktopTlsSpki = view.desktopTlsSpki,
            expiresAtMs = view.expiresAtMs,
            out = reencoded,
        )
        assertEquals(encoded.size, written)
        assertTrue(reencoded.copyOf(written).contentEquals(encoded))
    }

    @Test
    fun rejects_a_foreign_magic() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[0] = 'X'.code.toByte()
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(encoded) }
    }

    @Test
    fun rejects_an_unsupported_payload_version() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[4] = 2
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(encoded) }
    }

    @Test
    fun rejects_an_unsupported_protocol_version() {
        val encoded = hex(ENCODED_VECTOR_HEX)
        encoded[5] = 2
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(encoded) }
    }

    @Test
    fun rejects_a_zero_port_and_zero_expiry() {
        val zeroPort = hex(ENCODED_VECTOR_HEX)
        zeroPort[7] = 0
        zeroPort[8] = 0
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(zeroPort) }

        val zeroExpiry = hex(ENCODED_VECTOR_HEX)
        for (index in 59 until 67) {
            zeroExpiry[index] = 0
        }
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(zeroExpiry) }
    }

    @Test
    fun rejects_out_of_range_spki_lengths() {
        val shortSpki = hex(ENCODED_VECTOR_HEX)
        shortSpki[9] = 0
        shortSpki[10] = (PairingQr.SPKI_MIN_BYTES - 1).toByte()
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(shortSpki) }

        val longSpki = hex(ENCODED_VECTOR_HEX)
        val declared = PairingQr.SPKI_MAX_BYTES + 1
        longSpki[9] = (declared shr 8).toByte()
        longSpki[10] = declared.toByte()
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(longSpki) }
    }

    @Test
    fun rejects_length_mismatch_and_non_utf8_host() {
        val shiftedHost = hex(ENCODED_VECTOR_HEX)
        shiftedHost[6] = (shiftedHost[6].toInt() + 1).toByte()
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(shiftedHost) }

        val truncated = hex(ENCODED_VECTOR_HEX)
        val shortened = truncated.copyOf(truncated.size - 1)
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(shortened) }

        val nonUtf8Host = hex(ENCODED_VECTOR_HEX)
        nonUtf8Host[67] = 0xFF.toByte()
        assertThrows(IllegalArgumentException::class.java) { PairingQr.decode(nonUtf8Host) }
    }

    @Test
    fun encodes_the_canonical_fields() {
        val out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val written = PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = vectorHost,
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = vectorExpiryMs,
            out = out,
        )
        assertEquals(ENCODED_VECTOR_HEX.length / 2, written)
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_an_unknown_protocol_version_on_encode() {
        PairingQr.encode(
            protocolVersion = 2,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = vectorHost,
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = vectorExpiryMs,
            out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_a_zero_expiry_on_encode() {
        PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = vectorHost,
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = 0,
            out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_an_empty_host_on_encode() {
        PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = "",
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = vectorExpiryMs,
            out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_a_control_character_host_on_encode() {
        PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = "desktop\u007f.lab.local",
            controlPort = vectorPort,
            desktopTlsSpki = vectorSpki,
            expiresAtMs = vectorExpiryMs,
            out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES),
        )
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejects_a_short_spki_on_encode() {
        PairingQr.encode(
            protocolVersion = 1,
            pairingId = vectorPairingId,
            pairingSecret = vectorSecret,
            controlHost = vectorHost,
            controlPort = vectorPort,
            desktopTlsSpki = ByteArray(PairingQr.SPKI_MIN_BYTES - 1),
            expiresAtMs = vectorExpiryMs,
            out = ByteArray(PairingQr.MAX_PAYLOAD_BYTES),
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
            "534d5152010114b9fa0030000102030405060708090a0b0c0d0e0f" +
                "101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f" +
                "00000000001e84806465736b746f702d30312e6c61622e6c6f63616c" +
                "404142434445464748494a4b4c4d4e4f50515253545556575859" +
                "5a5b5c5d5e5f606162636465666768696a6b6c6d6e6f"
    }
}
