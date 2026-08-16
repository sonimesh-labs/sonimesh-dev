package com.sonimesh.sender.security

import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * JVM pin of the published media key derivation and nonce vectors.
 *
 * The golden bytes match the Rust pins in
 * `crates/sonimesh-protocol/src/media_key.rs` and the Python verifier
 * `tools/check_media_key_vectors.py` for both media directions, so the Android
 * implementation cannot drift from the cross-language media-plane boundary.
 */
class MediaKeyVectorTest {
    @Test
    fun matchesPublishedMediaKeyVector() {
        val key = deriveMediaKey(SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_ANDROID_TO_DESKTOP, SESSION_SECRET)
        assertArrayEquals(EXPECTED_MEDIA_KEY, key)
    }

    @Test
    fun matchesPublishedMediaNonceVector() {
        val nonce = mediaNonce(STREAM_ID, KEY_EPOCH, SEQUENCE, DIRECTION_ANDROID_TO_DESKTOP)
        assertArrayEquals(EXPECTED_MEDIA_NONCE, nonce)
    }

    @Test
    fun matchesPublishedReverseMediaKeyVector() {
        val key = deriveMediaKey(SESSION_ID, STREAM_ID, KEY_EPOCH, DIRECTION_DESKTOP_TO_ANDROID, SESSION_SECRET)
        assertArrayEquals(EXPECTED_REVERSE_MEDIA_KEY, key)
    }

    @Test
    fun matchesPublishedReverseMediaNonceVector() {
        val nonce = mediaNonce(STREAM_ID, KEY_EPOCH, SEQUENCE, DIRECTION_DESKTOP_TO_ANDROID)
        assertArrayEquals(EXPECTED_REVERSE_MEDIA_NONCE, nonce)
    }

    @Test
    fun nonceLayoutBindsEachDimensionWithoutByteOverlap() {
        val forward = mediaNonce(STREAM_ID, KEY_EPOCH, SEQUENCE, DIRECTION_ANDROID_TO_DESKTOP)
        assertEquals(KEY_EPOCH.toLong(), shortAt(forward, 0).toLong())
        assertEquals(DIRECTION_ANDROID_TO_DESKTOP.toLong(), forward[2].toLong())
        assertEquals(0L, forward[3].toLong())
        assertEquals(STREAM_ID.toLong(), intAt(forward, 4).toLong())
        assertEquals(SEQUENCE.toLong(), intAt(forward, 8).toLong())
        assertEquals(NONCE_BYTES, forward.size)

        val reverse = mediaNonce(STREAM_ID, KEY_EPOCH, SEQUENCE, DIRECTION_DESKTOP_TO_ANDROID)
        assertEquals(DIRECTION_DESKTOP_TO_ANDROID.toLong(), reverse[2].toLong())
        assertArrayEquals(forward.copyOfRange(0, 2), reverse.copyOfRange(0, 2))
        assertArrayEquals(forward.copyOfRange(4, NONCE_BYTES), reverse.copyOfRange(4, NONCE_BYTES))
    }

    private fun deriveMediaKey(
        sessionId: Long,
        streamId: Int,
        keyEpoch: Short,
        direction: Byte,
        sessionSecret: ByteArray,
    ): ByteArray {
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

    private fun shortAt(bytes: ByteArray, offset: Int): Short =
        ByteBuffer.wrap(bytes, offset, 2).order(ByteOrder.BIG_ENDIAN).short

    private fun intAt(bytes: ByteArray, offset: Int): Int =
        ByteBuffer.wrap(bytes, offset, 4).order(ByteOrder.BIG_ENDIAN).int

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        const val PROTOCOL_VERSION: Byte = 1
        const val MEDIA_KEY_BYTES = 32
        const val NONCE_BYTES = 12
        const val SESSION_ID = 7L
        const val STREAM_ID = 9
        const val KEY_EPOCH: Short = 3
        const val SEQUENCE = 1
        const val DIRECTION_ANDROID_TO_DESKTOP: Byte = 0
        const val DIRECTION_DESKTOP_TO_ANDROID: Byte = 1
        val ROOT_SALT = "sonimesh media root v1".encodeToByteArray()
        val STREAM_KEY_LABEL = "sonimesh media stream key v1".encodeToByteArray()
        val SESSION_SECRET = ByteArray(MEDIA_KEY_BYTES) { 4 }
        val EXPECTED_MEDIA_KEY = hex(
            "0B821644F0598E95080E4959E276FFCD" +
                "5ECFFF53775BCFC804CC17B23344D546",
        )
        val EXPECTED_MEDIA_NONCE = hex("000300000000000900000001")
        val EXPECTED_REVERSE_MEDIA_KEY = hex(
            "3EC355C7CDCE6A0AA0A439FFCD53A3E2" +
                "7A2A6A1CA0DFBC5832223AAEB094852C",
        )
        val EXPECTED_REVERSE_MEDIA_NONCE = hex("000301000000000900000001")
    }
}
