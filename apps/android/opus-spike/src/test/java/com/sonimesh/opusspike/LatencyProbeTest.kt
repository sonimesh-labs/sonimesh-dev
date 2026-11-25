package com.sonimesh.opusspike

import java.security.MessageDigest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LatencyProbeTest {
    @Test
    fun matchesTheDesktopGeneratorSignSequenceAndEndsInSilence() {
        val samples = latencyProbeSamples()
        assertEquals(LATENCY_PROBE_FRAMES, samples.size)
        assertTrue(samples.take(LATENCY_PROBE_PERIOD).all { it == 3_000.toShort() || it == (-3_000).toShort() })
        assertTrue(samples.drop(LATENCY_PROBE_PERIOD).all { it == 0.toShort() })
        val signs = samples.map { if (it > 0) 1.toByte() else if (it < 0) 2.toByte() else 0.toByte() }.toByteArray()
        val hash = MessageDigest.getInstance("SHA-256").digest(signs).joinToString("") { "%02x".format(it) }
        assertEquals("f26eb2b317894e29d9636dea1f31ddbfe3cbd9bdba0287a0ca2e2d541478c017", hash)
    }
}
