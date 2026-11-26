package com.sonimesh.opusspike

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class SenderGainTest {
    @Test
    fun countsOnlyAcceptedFramesClippedByGain() {
        val input = shortArrayOf(20_000, -20_000, 20_000, -20_000)
        val processor = SenderGainProcessor(input.size)
        processor.prepare(input, false, 200)
        processor.prepare(input, false, 200)
        assertEquals(0L, processor.clippedFrames)
        processor.accepted()
        assertEquals(1L, processor.clippedFrames)
        processor.prepare(input, true, 200)
        processor.accepted()
        assertEquals(1L, processor.clippedFrames)
        processor.prepare(shortArrayOf(Short.MAX_VALUE, Short.MIN_VALUE, Short.MAX_VALUE, Short.MIN_VALUE), false, 100)
        processor.accepted()
        assertEquals(1L, processor.clippedFrames)
    }
    @Test
    fun scalesAndClipsWithoutChangingCapturedInput() {
        val input = shortArrayOf(20_000, -20_000, 10_000, -10_000)
        val output = ShortArray(input.size)
        assertSame(input, scaleSenderPcm(input, output, 100, 100))
        assertArrayEquals(shortArrayOf(10_000, -10_000, 5_000, -5_000), scaleSenderPcm(input, output, 50, 50))
        assertArrayEquals(shortArrayOf(Short.MAX_VALUE, Short.MIN_VALUE, 20_000, -20_000), scaleSenderPcm(input, output, 200, 200))
        assertArrayEquals(shortArrayOf(0, 0, 0, 0), scaleSenderPcm(input, output, 0, 0))
        assertArrayEquals(shortArrayOf(20_000, -20_000, 10_000, -10_000), input)
    }

    @Test
    fun rejectsUnsupportedGainAndBufferSizes() {
        assertThrows(IllegalArgumentException::class.java) { scaleSenderPcm(ShortArray(4), ShortArray(4), 100, 201) }
        assertThrows(IllegalArgumentException::class.java) { scaleSenderPcm(ShortArray(4), ShortArray(0), 100, 100) }
    }

    @Test
    fun rampsBothStereoChannelsAndAdvancesOnlyAfterAcceptance() {
        val input = shortArrayOf(10_000, -10_000, 10_000, -10_000, 10_000, -10_000)
        val processor = SenderGainProcessor(input.size)
        val ramp = shortArrayOf(10_000, -10_000, 15_000, -15_000, 20_000, -20_000)
        assertArrayEquals(ramp, processor.prepare(input, false, 200))
        assertArrayEquals(ramp, processor.prepare(input, false, 200))
        assertArrayEquals(shortArrayOf(0, 0, 0, 0, 0, 0), processor.prepare(input, true, 200))
        assertArrayEquals(ramp, processor.prepare(input, false, 200))
        processor.accepted()
        assertArrayEquals(shortArrayOf(20_000, -20_000, 20_000, -20_000, 20_000, -20_000),
            processor.prepare(input, false, 200))
        assertArrayEquals(shortArrayOf(20_000, -20_000, 15_000, -15_000, 10_000, -10_000),
            processor.prepare(input, false, 100))
    }
}
