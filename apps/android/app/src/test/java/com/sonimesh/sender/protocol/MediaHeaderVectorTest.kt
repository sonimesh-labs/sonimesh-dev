package com.sonimesh.sender.protocol

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

class MediaHeaderVectorTest {
    @Test
    fun matchesPublishedMediaHeaderVector() {
        val encoded = mediaHeader(
            flags = FLAGS,
            sessionId = SESSION_ID,
            streamId = STREAM_ID,
            sequence = SEQUENCE,
            sampleTimestamp = SAMPLE_TIMESTAMP,
            payloadBytes = PAYLOAD_BYTES,
            keyEpoch = KEY_EPOCH,
        )

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

    private fun mediaHeader(
        flags: Byte,
        sessionId: Long,
        streamId: Int,
        sequence: Int,
        sampleTimestamp: Long,
        payloadBytes: Int,
        keyEpoch: Short,
    ): ByteArray = ByteBuffer.allocate(MEDIA_HEADER_LEN)
        .order(ByteOrder.BIG_ENDIAN)
        .putInt(MEDIA_MAGIC)
        .put(MEDIA_VERSION)
        .put(flags)
        .putShort(MEDIA_HEADER_LEN.toShort())
        .putLong(sessionId)
        .putInt(streamId)
        .putInt(sequence)
        .putLong(sampleTimestamp)
        .putShort(payloadBytes.toShort())
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
        const val MEDIA_VERSION: Byte = 1
        const val FLAGS: Byte = 0
        const val MEDIA_HEADER_LEN = 36
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