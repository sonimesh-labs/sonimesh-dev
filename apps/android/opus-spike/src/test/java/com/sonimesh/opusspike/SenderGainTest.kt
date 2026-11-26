package com.sonimesh.opusspike

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class SenderGainTest {
    @Test
    fun scalesAndClipsWithoutChangingCapturedInput() {
        val input = shortArrayOf(20_000, -20_000, 10_000, -10_000)
        val output = ShortArray(input.size)
        assertSame(input, scaleSenderPcm(input, output, 100))
        assertArrayEquals(shortArrayOf(10_000, -10_000, 5_000, -5_000), scaleSenderPcm(input, output, 50))
        assertArrayEquals(shortArrayOf(Short.MAX_VALUE, Short.MIN_VALUE, 20_000, -20_000), scaleSenderPcm(input, output, 200))
        assertArrayEquals(shortArrayOf(0, 0, 0, 0), scaleSenderPcm(input, output, 0))
        assertArrayEquals(shortArrayOf(20_000, -20_000, 10_000, -10_000), input)
    }

    @Test
    fun rejectsUnsupportedGainAndBufferSizes() {
        assertThrows(IllegalArgumentException::class.java) { scaleSenderPcm(shortArrayOf(1), ShortArray(1), 201) }
        assertThrows(IllegalArgumentException::class.java) { scaleSenderPcm(shortArrayOf(1), ShortArray(0), 100) }
    }
}
