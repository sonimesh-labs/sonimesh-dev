package com.sonimesh.sender.protocol

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class MediaHeaderVectorTest {
    @Test
    fun matchesPublishedMediaHeaderVector() {
        val encoded = header().encode(PAYLOAD_BYTES)

        assertArrayEquals(EXPECTED_HEADER, encoded)
        assertEquals(MEDIA_HEADER_LEN, encoded.size)
    }

    @Test
    fun publishedLayoutMatchesDocumentedConstants() {
        val buffer = ByteBuffer.wrap(EXPECTED_HEADER).order(ByteOrder.BIG_ENDIAN)
        assertEquals(MEDIA_MAGIC.toLong(), buffer.int.toLong())
        assertEquals(MEDIA_VERSION.toLong(), buffer.get().toLong())
        assertEquals(FLAGS.toLong(), buffer.get().toLong())
        assertEquals(MEDIA_HEADER_LEN.toLong(), buffer.short.toLong())
        assertEquals(SESSION_ID, buffer.long)
        assertEquals(STREAM_ID.toLong(), buffer.int.toLong())
        assertEquals(SEQUENCE.toLong(), buffer.int.toLong())
        assertEquals(SAMPLE_TIMESTAMP, buffer.long)
        assertEquals(PAYLOAD_BYTES.toLong(), buffer.short.toLong())
        assertEquals(KEY_EPOCH.toLong(), buffer.short.toLong())
    }

    @Test
    fun encodeAcceptsTheMaximumEncryptedPayload() {
        val encoded = header().encode(MAX_ENCRYPTED_PAYLOAD_LEN)

        assertEquals(MEDIA_HEADER_LEN, encoded.size)
        assertEquals(
            MAX_MEDIA_DATAGRAM_LEN,
            MEDIA_HEADER_LEN + MAX_ENCRYPTED_PAYLOAD_LEN,
        )
    }

    @Test
    fun encodeRejectsPayloadAboveTheHardLimit() {
        assertThrows(IllegalArgumentException::class.java) {
            header().encode(MAX_ENCRYPTED_PAYLOAD_LEN + 1)
        }
    }

    @Test
    fun encodeRejectsNegativePayload() {
        assertThrows(IllegalArgumentException::class.java) {
            header().encode(-1)
        }
    }

    private fun header() = MediaHeader(
        flags = FLAGS,
        sessionId = SESSION_ID,
        streamId = STREAM_ID,
        sequence = SEQUENCE,
        sampleTimestamp = SAMPLE_TIMESTAMP,
        keyEpoch = KEY_EPOCH,
    )

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        const val FLAGS: Byte = 0
        const val SESSION_ID = 0x0102030405060708L
        const val STREAM_ID = 9
        const val SEQUENCE = 7
        const val SAMPLE_TIMESTAMP = 2_880L
        const val PAYLOAD_BYTES = 64
        const val KEY_EPOCH: Short = 0x2345
        val EXPECTED_HEADER = hex(
            "534E4D5301000024010203040506070800000009000000070000000000000B4000402345",
        )
    }
}
