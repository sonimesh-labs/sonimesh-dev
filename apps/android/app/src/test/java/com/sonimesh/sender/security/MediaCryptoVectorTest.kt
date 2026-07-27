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
        val key = deriveMediaKey()
        val nonce = mediaNonce()
        val header = mediaHeader()
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
    fun decrypts_the_published_vector_and_rejects_tampering() {
        assertArrayEquals(PLAINTEXT, decrypt(EXPECTED_DATAGRAM))

        val tamperedHeader = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[FLAGS_OFFSET] = (datagram[FLAGS_OFFSET].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedHeader)
        }

        val tamperedCiphertext = EXPECTED_DATAGRAM.copyOf().also { datagram ->
            datagram[HEADER_BYTES] = (datagram[HEADER_BYTES].toInt() xor 0x01).toByte()
        }
        assertThrows(GeneralSecurityException::class.java) {
            decrypt(tamperedCiphertext)
        }
    }

    private fun decrypt(datagram: ByteArray): ByteArray {
        require(datagram.size >= HEADER_BYTES + POLY1305_TAG_BYTES)
        val header = datagram.copyOfRange(0, HEADER_BYTES)
        val ciphertext = datagram.copyOfRange(HEADER_BYTES, datagram.size)
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(
            Cipher.DECRYPT_MODE,
            SecretKeySpec(deriveMediaKey(), "ChaCha20"),
            IvParameterSpec(mediaNonce()),
        )
        cipher.updateAAD(header)
        return cipher.doFinal(ciphertext)
    }

    private fun deriveMediaKey(): ByteArray {
        val extract = Mac.getInstance("HmacSHA256")
        extract.init(SecretKeySpec(ROOT_SALT, "HmacSHA256"))
        val pseudorandomKey = extract.doFinal(SESSION_SECRET)

        val info = ByteBuffer.allocate(STREAM_KEY_LABEL.size + 1 + 8 + 4 + 2 + 1)
            .order(ByteOrder.BIG_ENDIAN)
            .put(STREAM_KEY_LABEL)
            .put(PROTOCOL_VERSION)
            .putLong(SESSION_ID)
            .putInt(STREAM_ID)
            .putShort(KEY_EPOCH)
            .put(DIRECTION_ANDROID_TO_DESKTOP)
            .array()
        val expand = Mac.getInstance("HmacSHA256")
        expand.init(SecretKeySpec(pseudorandomKey, "HmacSHA256"))
        return expand.doFinal(info + byteArrayOf(1)).copyOf(MEDIA_KEY_BYTES)
    }

    private fun mediaNonce(): ByteArray = ByteBuffer.allocate(NONCE_BYTES)
        .order(ByteOrder.BIG_ENDIAN)
        .putShort(KEY_EPOCH)
        .put(DIRECTION_ANDROID_TO_DESKTOP)
        .put(0)
        .putInt(STREAM_ID)
        .putInt(SEQUENCE)
        .array()

    private fun mediaHeader(): ByteArray = ByteBuffer.allocate(HEADER_BYTES)
        .order(ByteOrder.BIG_ENDIAN)
        .putInt(MEDIA_MAGIC)
        .put(PROTOCOL_VERSION)
        .put(FLAGS)
        .putShort(HEADER_BYTES.toShort())
        .putLong(SESSION_ID)
        .putInt(STREAM_ID)
        .putInt(SEQUENCE)
        .putLong(SAMPLE_TIMESTAMP)
        .putShort((PLAINTEXT.size + POLY1305_TAG_BYTES).toShort())
        .putShort(KEY_EPOCH)
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
        val ROOT_SALT = "sonimesh media root v1".encodeToByteArray()
        val STREAM_KEY_LABEL = "sonimesh media stream key v1".encodeToByteArray()
        val SESSION_SECRET = hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
        val PLAINTEXT = "SoniMesh media vector".encodeToByteArray()
        val EXPECTED_NONCE = hex("0e0f00000a0b0c0d00000000")
        val EXPECTED_DATAGRAM = hex(
            "534e4d530102002401020304050607080a0b0c0d0000000000000000000003c0" +
                "00250e0f7cce62ea960fc27cf5a6ffce2aa59343a5f12997791396ca10bdfc7c" +
                "c060c9dfe49fd67131",
        )
    }
}
