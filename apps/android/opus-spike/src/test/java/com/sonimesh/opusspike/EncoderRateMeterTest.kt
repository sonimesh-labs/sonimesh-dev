package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EncoderRateMeterTest {
    @Test
    fun reportsCumulativePayloadOverElapsedTime() {
        val meter = EncoderRateMeter(100)
        assertNull(meter.averageKbps(2_100))
        meter.record(8_000)
        assertNull(meter.averageKbps(1_099))
        assertEquals(64L, meter.averageKbps(1_100))
        meter.record(4_000)
        assertEquals(48L, meter.averageKbps(2_100))
        assertNull(meter.averageKbps(50))
    }
}
