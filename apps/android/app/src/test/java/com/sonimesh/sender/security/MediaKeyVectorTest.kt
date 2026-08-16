package com.sonimesh.sender.security

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * JVM pin of the published media key derivation and nonce vectors.
 *
 * The golden bytes match the Rust pins in
 * `crates/sonimesh-protocol/src/media_key.rs` and the Python verifier
 * `tools/check_media_key_vectors.py` for both media directions, so the Android
 * implementation cannot drift from the cross-language media-plane boundary.
 * The key and nonce are derived through the `MediaKey` mirror.
 */
class MediaKeyVectorTest {
    @Test
    fun matchesPublishedMediaKeyVector() {
        val key = MediaKey.deriveMediaKey(SESSION_SECRET, forwardContext())
        assertArrayEquals(EXPECTED_MEDIA_KEY, key)
    }

    @Test
    fun matchesPublishedMediaNonceVector() {
        val nonce = MediaKey.mediaNonce(forwardContext(), SEQUENCE)
        assertArrayEquals(EXPECTED_MEDIA_NONCE, nonce)
    }

    @Test
    fun matchesPublishedReverseMediaKeyVector() {
        val key = MediaKey.deriveMediaKey(SESSION_SECRET, reverseContext())
        assertArrayEquals(EXPECTED_REVERSE_MEDIA_KEY, key)
    }

    @Test
    fun matchesPublishedReverseMediaNonceVector() {
        val nonce = MediaKey.mediaNonce(reverseContext(), SEQUENCE)
        assertArrayEquals(EXPECTED_REVERSE_MEDIA_NONCE, nonce)
    }

    @Test
    fun nonceLayoutBindsEachDimensionWithoutByteOverlap() {
        val forward = MediaKey.mediaNonce(forwardContext(), SEQUENCE)
        assertEquals(KEY_EPOCH.toLong(), shortAt(forward, 0).toLong())
        assertEquals(MediaDirection.ANDROID_TO_DESKTOP.byte.toLong(), forward[2].toLong())
        assertEquals(0L, forward[3].toLong())
        assertEquals(STREAM_ID.toLong(), intAt(forward, 4).toLong())
        assertEquals(SEQUENCE.toLong(), intAt(forward, 8).toLong())
        assertEquals(NONCE_BYTES, forward.size)

        val reverse = MediaKey.mediaNonce(reverseContext(), SEQUENCE)
        assertEquals(MediaDirection.DESKTOP_TO_ANDROID.byte.toLong(), reverse[2].toLong())
        assertArrayEquals(forward.copyOfRange(0, 2), reverse.copyOfRange(0, 2))
        assertArrayEquals(forward.copyOfRange(4, NONCE_BYTES), reverse.copyOfRange(4, NONCE_BYTES))
    }

    @Test
    fun derivesDeterministicallyAndSeparatesDirection() {
        val forward = MediaKey.deriveMediaKey(SESSION_SECRET, forwardContext())
        val same = MediaKey.deriveMediaKey(SESSION_SECRET, forwardContext())
        val reverse = MediaKey.deriveMediaKey(SESSION_SECRET, reverseContext())

        assertArrayEquals(forward, same)
        assertNotEquals(forward.toList(), reverse.toList())
    }

    @Test
    fun mediaKeyContextRejectsZeroSessionOrStream() {
        assertThrows(IllegalArgumentException::class.java) {
            MediaKeyContext(
                sessionId = 0L,
                streamId = STREAM_ID,
                keyEpoch = KEY_EPOCH,
                direction = MediaDirection.ANDROID_TO_DESKTOP,
            )
        }
        assertThrows(IllegalArgumentException::class.java) {
            MediaKeyContext(
                sessionId = SESSION_ID,
                streamId = 0,
                keyEpoch = KEY_EPOCH,
                direction = MediaDirection.ANDROID_TO_DESKTOP,
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
        sessionId = SESSION_ID,
        streamId = STREAM_ID,
        keyEpoch = KEY_EPOCH,
        direction = MediaDirection.DESKTOP_TO_ANDROID,
    )

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

        const val SESSION_ID = 7L
        const val STREAM_ID = 9
        const val KEY_EPOCH: Short = 3
        const val SEQUENCE = 1
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
