package com.sonimesh.sender

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureTestToneTest {
    @Test
    fun tone_is_short_quiet_and_loops_near_zero_for_supported_rates() {
        for (sampleRate in listOf(48_000, 44_100)) {
            val samples = captureTestToneSamples(sampleRate)
            assertEquals(sampleRate / 10, samples.size)
            assertEquals(0, samples.first().toInt())
            assertTrue(samples.any { it > 2_900 })
            assertTrue(samples.any { it < -2_900 })
            assertTrue(samples.all { it in -3_000..3_000 })
            assertTrue(kotlin.math.abs(samples.last().toInt()) < 200)
        }
    }
}
