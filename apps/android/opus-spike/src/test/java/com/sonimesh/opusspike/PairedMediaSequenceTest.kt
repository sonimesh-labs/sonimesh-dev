package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

class PairedMediaSequenceTest {
    @Test
    fun newSessionStartsAtZeroAndPreservesDroppedFrames() {
        val firstSession = PairedMediaSequence()
        assertEquals(0L, firstSession.next(0))
        assertEquals(2L, firstSession.next(2))

        val reconnected = PairedMediaSequence()
        assertEquals(0L, reconnected.next(1_250))
        assertEquals(1L, reconnected.next(1_251))
        assertEquals(4L, reconnected.next(1_254))
    }

    @Test
    fun rejectsReusedOrExhaustedSequence() {
        val session = PairedMediaSequence()
        assertEquals(0L, session.next(100))
        assertThrows(IllegalArgumentException::class.java) { session.next(100) }
        assertThrows(IllegalArgumentException::class.java) { session.next(99) }
        assertThrows(IllegalArgumentException::class.java) { session.next(100 + 0x1_0000_0000L) }
        assertEquals(1L, session.next(101))
    }

    @Test
    fun reconnectedPacketsCarrySessionRelativeSequenceAndTimestamp() {
        val sequence = PairedMediaSequence()
        val sealer = LabMediaSealer(ByteArray(32) { 1 }, 42, 1, 0)
        try {
            for ((encodedSequence, expected) in listOf(1_250L to 0L, 1_253L to 3L)) {
                val relative = sequence.next(encodedSequence)
                val packet = sealer.seal(relative, relative * 960, 0, byteArrayOf(1))
                val header = ByteBuffer.wrap(packet).order(ByteOrder.BIG_ENDIAN)
                assertEquals(expected, header.getInt(20).toLong() and 0xffff_ffffL)
                assertEquals(expected * 960, header.getLong(24))
            }
        } finally {
            sealer.clear()
        }
    }
}
