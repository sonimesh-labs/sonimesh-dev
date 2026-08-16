package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.MAX_ENCRYPTED_PAYLOAD_LEN
import com.sonimesh.sender.protocol.MEDIA_HEADER_LEN
import java.security.GeneralSecurityException
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class MediaCryptoVectorTest {
    @Test
    fun pinsTheFixedCryptoBounds() {
        assertEquals(16, POLY1305_TAG_BYTES)
        assertEquals(
            MAX_ENCRYPTED_PAYLOAD_LEN - POLY1305_TAG_BYTES,
            MAX_MEDIA_PLAINTEXT_LEN,
        )
    }

    @Test
    fun matches_published_media_datagram_vector() {
        val context = forwardContext()
        assertArrayEquals(EXPECTED_NONCE, MediaKey.mediaNonce(context, SEQUENCE))
        assertArrayEquals(
            EXPECTED_DATAGRAM,
            MediaCrypto.sealDatagram(
                sessionSecret = SESSION_SECRET,
                context = context,
                flags = FLAGS,
                sampleTimestamp = SAMPLE_TIMESTAMP,
                sequence = SEQUENCE,
                plaintext = PLAINTEXT,
            ),
        )
    }

    @Test
    fun matches_published_reverse_direction_vector() {
        val context = reverseContext()
        assertArrayEquals(EXPECTED_REVERSE_NONCE, MediaKey.mediaNonce(context, REVERSE_SEQUENCE))
        assertArrayEquals(
            EXPECTED_REVERSE_DATAGRAM,
            MediaCrypto.sealDatagram(
                sessionSecret = REVERSE_SESSION_SECRET,
                context = context,
                flags = REVERSE_FLAGS,
                sampleTimestamp = REVERSE_SAMPLE_TIMESTAMP,
                sequence = REVERSE_SEQUENCE,
                plaintext = REVERSE_PLAINTEXT,
            ),
        )
    }

    @Test
    fun decrypts_the_published_vector_and_rejects_tampering() {
        assertArrayEquals(
            PLAINTEXT,
            MediaCrypto.openDatagram(SESSION_SECRET, forwardContext(), EXPECTED_DATAGRAM),
        )

        val tamperedHeader = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[FLAGS_OFFSET] = (datagram[FLAGS_OFFSET].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            MediaCrypto.openDatagram(SESSION_SECRET, forwardContext(), tamperedHeader)
        }

        val tamperedCiphertext = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[MEDIA_HEADER_LEN] = (datagram[MEDIA_HEADER_LEN].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            MediaCrypto.openDatagram(SESSION_SECRET, forwardContext(), tamperedCiphertext)
        }
    }

    @Test
    fun decrypts_the_reverse_vector_and_rejects_tampering() {
        assertArrayEquals(
            REVERSE_PLAINTEXT,
            MediaCrypto.openDatagram(REVERSE_SESSION_SECRET, reverseContext(), EXPECTED_REVERSE_DATAGRAM),
        )

        val tamperedHeader = EXPECTED_REVERSE_DATAGRAM.copyOf().also { datagram ->
            datagram[FLAGS_OFFSET] = (datagram[FLAGS_OFFSET].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            MediaCrypto.openDatagram(REVERSE_SESSION_SECRET, reverseContext(), tamperedHeader)
        }

        val tamperedCiphertext = EXPECTED_REVERSE_DATAGRAM.copyOf().also { datagram ->
            datagram[MEDIA_HEADER_LEN] = (datagram[MEDIA_HEADER_LEN].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            MediaCrypto.openDatagram(REVERSE_SESSION_SECRET, reverseContext(), tamperedCiphertext)
        }
    }

    @Test
    fun sealRejectsPlaintextAboveTheHardLimit() {
        assertThrows(IllegalArgumentException::class.java) {
            MediaCrypto.sealDatagram(
                sessionSecret = SESSION_SECRET,
                context = forwardContext(),
                flags = FLAGS,
                sampleTimestamp = SAMPLE_TIMESTAMP,
                sequence = SEQUENCE,
                plaintext = ByteArray(MAX_MEDIA_PLAINTEXT_LEN + 1),
            )
        }
    }

    @Test
    fun openRejectsTruncatedDatagram() {
        assertThrows(IllegalArgumentException::class.java) {
            MediaCrypto.openDatagram(
                sessionSecret = SESSION_SECRET,
                context = forwardContext(),
                datagram = ByteArray(MEDIA_HEADER_LEN + POLY1305_TAG_BYTES - 1),
            )
        }
    }

    private fun forwardContext() = MediaKeyContext(
        sessionId = SESSION_ID,
        streamId = STREAM_ID,
        keyEpoch = KEY_EPOCH,
        direction = MediaDirection.ANDROID_TO_DESKTOP,
    )

    private fun reverseContext() = MediaKeyContext(
        sessionId = REVERSE_SESSION_ID,
        streamId = REVERSE_STREAM_ID,
        keyEpoch = REVERSE_KEY_EPOCH,
        direction = MediaDirection.DESKTOP_TO_ANDROID,
    )

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        const val FLAGS: Byte = 0x02
        const val FLAGS_OFFSET = 5
        const val SESSION_ID = 0x0102030405060708L
        const val STREAM_ID = 0x0A0B0C0D
        const val SEQUENCE = 0
        const val SAMPLE_TIMESTAMP = 960L
        const val KEY_EPOCH: Short = 0x0E0F
        const val REVERSE_FLAGS: Byte = 0x00
        const val REVERSE_SESSION_ID = 0x0F0E0D0C0B0A0908L
        const val REVERSE_STREAM_ID = 0x12345678
        const val REVERSE_SEQUENCE = 0
        const val REVERSE_KEY_EPOCH: Short = 0x002A
        // 0x8899AABBCCDDEEFF exceeds Long.MAX_VALUE; preserve the bit pattern.
        val REVERSE_SAMPLE_TIMESTAMP = 0x8899AABBCCDDEEFFuL.toLong()
        val SESSION_SECRET = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
        val REVERSE_SESSION_SECRET = hex("101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f")
        val PLAINTEXT = "SoniMesh media vector".encodeToByteArray()
        val REVERSE_PLAINTEXT = "SoniMesh reverse direction vector".encodeToByteArray()
        val EXPECTED_NONCE = hex("0e0f00000a0b0c0d00000000")
        val EXPECTED_REVERSE_NONCE = hex("002a01001234567800000000")
        val EXPECTED_DATAGRAM = hex(
            "534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0" +
                "00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c" +
                "c060c9dfe49fd67131",
        )
        val EXPECTED_REVERSE_DATAGRAM = hex(
            "534E4D53010000240F0E0D0C0B0A090812345678000000008899AABBCCDDEEFF" +
                "0031002AA0853360825EFA8AB1529900AA2403A2E82B06C3C8FBB2B80B20539" +
                "3DFFF19159430B6A033E699E1613D8689455DC95A99",
        )
    }
}
