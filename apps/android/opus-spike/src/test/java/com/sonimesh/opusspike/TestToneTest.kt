package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

class TestToneTest {
    @Test
    fun supportedFormatsProduceOneSecondOfContinuousOneKilohertzTone() {
        for (sampleRate in listOf(48_000, 44_100)) {
            for (channels in listOf(1, 2)) {
                val samples = testToneSamples(sampleRate, channels)
                assertEquals(sampleRate * channels, samples.size)
                val frames = samples.toList().chunked(channels)
                assertTrue(frames.all { frame -> frame.all { it == frame.first() } })
                val mono = frames.map { it.first().toInt() }
                val upwardCrossings = mono.indices.count { index ->
                    mono[index] <= 0 && mono[(index + 1) % sampleRate] > 0
                }
                assertEquals(1_000, upwardCrossings)
                val largestStep = mono.indices.maxOf { index ->
                    abs(mono[index] - mono[(index + 1) % sampleRate])
                }
                assertTrue("loop must not introduce a large jump", largestStep < 500)
            }
        }
    }
}
