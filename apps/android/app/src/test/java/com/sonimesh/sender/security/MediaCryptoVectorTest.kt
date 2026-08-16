package com.sonimesh.sender.security

import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class MediaCryptoVectorTest {
    @Test
    fun matches_published_media_datagram_vector() {
        val key = deriveMediaKey(SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_ANDROID_TO_DESKTOP, SESSION_SECRET)
        val nonce = mediaNonce(STREAM_ID, KEY_EPOCH, SEQUENCE, DIRECTION_ANDROID_TO_DESKTOP)
        val header = mediaHeader(FLAGS, SESSION_ID, STREAM_ID, SEQUENCE, SAMPLE_TIMESTAMP, PLAINTEXT.size, KEY_EPOCH)
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(
            Cipher.ENCRYPT_MODE,
            SecretKeySpec(key, "ChaCha20"),
            IvParameterSpec(nonce),
        )
        cipher.updateAAD(header)

        assertArrayEquals(
            EXPECTED_NONCE,
            nonce,
        )
        assertArrayEquals(
            EXPECTED_DATAGRAM,
            header + cipher.doFinal(PLAINTEXT),
        )
    }

    @Test
    fun matches_published_reverse_direction_vector() {
        val key = deriveMediaKey(REVERSE_SESSION_ID, REVERSE_STREAM_ID, REVERSE_KEY_EPOCH, DIRECTION_DESKTOP_TO_ANDROID, REVERSE_SESSION_SECRET)
        val nonce = mediaNonce(REVERSE_STREAM_ID, REVERSE_KEY_EPOCH, REVERSE_SEQUENCE, DIRECTION_DESKTOP_TO_ANDROID)
        val header = mediaHeader(REVERSE_FLAGS, REVERSE_SESSION_ID, REVERSE_STREAM_ID, REVERSE_SEQUENCE, REVERSE_SAMPLE_TIMESTAMP, REVERSE_PLAINTEXT.size, REVERSE_KEY_EPOCH)
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(
            Cipher.ENCRYPT_MODE,
            SecretKeySpec(key, "ChaCha20"),
            IvParameterSpec(nonce),
        )
        cipher.updateAAD(header)

        assertArrayEquals(
            EXPECTED_REVERSE_NONCE,
            nonce,
        )
        assertArrayEquals(
            EXPECTED_REVERSE_DATAGRAM,
            header + cipher.doFinal(REVERSE_PLAINTEXT),
        )
    }

    @Test
    fun decrypts_the_published_vector_and_rejects_tampering() {
        assertArrayEquals(PLAINTEXT, decrypt(EXPECTED_DATAGRAM, SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_ANDROID_TO_DESKTOP, SESSION_SECRET))

        val tamperedHeader = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[FLAGS_OFFSET] = (datagram[FLAGS_OFFSET].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedHeader, SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_ANDROID_TO_DESKTOP, SESSION_SECRET)
        }

        val tamperedCiphertext = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[HEADER_BYTES] = (datagram[HEADER_BYTES].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedCiphertext, SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_ANDROID_TO_DESKTOP, SESSION_SECRET)
        }
    }

    @Test
    fun decrypts_the_reverse_vector_and_rejects_tampering() {
        assertArrayEquals(REVERSE_PLAINTEXT, decrypt(EXPECTED_REVERSE_DATAGRAM, REVERSE_SESSION_ID, REVERSE_STREAM_ID, REVERSE_KEY_EPOCH, DIRECTION_DESKTOP_TO_ANDROID, REVERSE_SESSION_SECRET))

        val tamperedHeader = EXPECTED_REVERSE_DATAGRAM.copyOf().also { datagram ->
            datagram[FLAGS_OFFSET] = (datagram[FLAGS_OFFSET].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedHeader, REVERSE_SESSION_ID, REVERSE_STREAM_ID, REVERSE_KEY_EPOCH, DIRECTION_DESKTOP_TO_ANDROID, REVERSE_SESSION_SECRET)
        }

        val tamperedCiphertext = EXPECTED_REVERSE_DATAGRAM.copyOf().also { datagram ->
            datagram[HEADER_BYTES] = (datagram[HEADER_BYTES].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedCiphertext, REVERSE_SESSION_ID, REVERSE_STREAM_ID, REVERSE_KEY_EPOCH, DIRECTION_DESKTOP_TO_ANDROID, REVERSE_SESSION_SECRET)
        }
    }

    private fun decrypt(
        datagram: ByteArray,
        sessionId: Long,
        streamId: Int,
        keyEpoch: Short,
        direction: Byte,
        sessionSecret: ByteArray,
    ): ByteArray {
        require(datagram.size >= HEADER_BYTES + POLY1305_TAG_BYTES)
        val header = datagram.copyOfRange(0, HEADER_BYTES)
        val ciphertext = datagram.copyOfRange(HEADER_BYTES, datagram.size)
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(
            Cipher.DECRYPT_MODE,
            SecretKeySpec(deriveMediaKey(sessionId, streamId, keyEpoch, direction, sessionSecret), "ChaCha20"),
            IvParameterSpec(mediaNonce(streamId, keyEpoch, 0, direction)),
        )
        cipher.updateAAD(header)
        return cipher.doFinal(ciphertext)
    }

    private fun deriveMediaKey(sessionId: Long, streamId: Int, keyEpoch: Short, direction: Byte, sessionSecret: ByteArray): ByteArray {
        val extract = Mac.getInstance("HmacSHA256")
        extract.init(SecretKeySpec(ROOT_SALT, "HmacSHA256"))
        val pseudorandomKey = extract.doFinal(sessionSecret)

        val info = ByteBuffer.allocate(STREAM_KEY_LABEL.size + 1 + 8 + 4 + 2 + 1)
            .order(ByteOrder.BIG_ENDIAN)
            .put(STREAM_KEY_LABEL)
            .put(PROTOCOL_VERSION)
            .putLong(sessionId)
            .putInt(streamId)
            .putShort(keyEpoch)
            .put(direction)
            .array()
        val expand = Mac.getInstance("HmacSHA256")
        expand.init(SecretKeySpec(pseudorandomKey, "HmacSHA256"))
        return expand.doFinal(info + byteArrayOf(1)).copyOf(MEDIA_KEY_BYTES)
    }

    private fun mediaNonce(streamId: Int, keyEpoch: Short, sequence: Int, direction: Byte): ByteArray =
        ByteBuffer.allocate(NONCE_BYTES)
            .order(ByteOrder.BIG_ENDIAN)
            .putShort(keyEpoch)
            .put(direction)
            .put(0)
            .putInt(streamId)
            .putInt(sequence)
            .array()

    private fun mediaHeader(
        flags: Byte,
        sessionId: Long,
        streamId: Int,
        sequence: Int,
        sampleTimestamp: Long,
        plaintextBytes: Int,
        keyEpoch: Short,
    ): ByteArray = ByteBuffer.allocate(HEADER_BYTES)
        .order(ByteOrder.BIG_ENDIAN)
        .putInt(MEDIA_MAGIC)
        .put(PROTOCOL_VERSION)
        .put(flags)
        .putShort(HEADER_BYTES.toShort())
        .putLong(sessionId)
        .putInt(streamId)
        .putInt(sequence)
        .putLong(sampleTimestamp)
        .putShort((plaintextBytes + POLY1305_TAG_BYTES).toShort())
        .putShort(keyEpoch)
        .array()

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        const val MEDIA_MAGIC = 0x534E4D53
        const val PROTOCOL_VERSION: Byte = 1
        const val FLAGS: Byte = 0x02
        const val FLAGS_OFFSET = 5
        const val HEADER_BYTES = 36
        const val POLY1305_TAG_BYTES = 16
        const val MEDIA_KEY_BYTES = 32
        const val NONCE_BYTES = 12
        const val SESSION_ID = 0x0102030405060708L
        const val STREAM_ID = 0x0A0B0C0D
        const val SEQUENCE = 0
        const val SAMPLE_TIMESTAMP = 960L
        const val KEY_EPOCH: Short = 0x0E0F
        const val DIRECTION_ANDROID_TO_DESKTOP: Byte = 0
        const val REVERSE_FLAGS: Byte = 0x00
        const val DIRECTION_DESKTOP_TO_ANDROID: Byte = 1
        const val REVERSE_SESSION_ID = 0x0F0E0D0C0B0A0908L
        const val REVERSE_STREAM_ID = 0x12345678
        const val REVERSE_SEQUENCE = 0
        const val REVERSE_KEY_EPOCH: Short = 0x002A
        // 0x8899AABBCCDDEEFF exceeds Long.MAX_VALUE; preserve the bit pattern.
        val REVERSE_SAMPLE_TIMESTAMP = 0x8899AABBCCDDEEFFuL.toLong()
        val ROOT_SALT = "sonimesh media root v1".encodeToByteArray()
        val STREAM_KEY_LABEL = "sonimesh media stream key v1".encodeToByteArray()
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
