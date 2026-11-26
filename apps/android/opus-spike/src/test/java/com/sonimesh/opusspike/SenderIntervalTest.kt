package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SenderIntervalTest {
    @Test
    fun reportsSendingAndIdleIntervalsWithTheirActualDuration() {
        val meter = SenderIntervalMeter(100)
        assertNull(meter.sample(1_099, 49))
        assertEquals(SenderInterval(50, 1_000), meter.sample(1_100, 50))
        assertEquals(SenderInterval(0, 1_400), meter.sample(2_500, 50))
        assertEquals(SenderInterval(12, 1_000), meter.sample(3_500, 62))
    }

    @Test
    fun rebasesAfterClockOrCounterRegression() {
        val meter = SenderIntervalMeter(100)
        assertEquals(SenderInterval(10, 1_000), meter.sample(1_100, 10))
        assertNull(meter.sample(1_000, 10))
        assertEquals(SenderInterval(3, 1_000), meter.sample(2_000, 13))
        assertNull(meter.sample(2_100, 0))
        assertEquals(SenderInterval(2, 1_000), meter.sample(3_100, 2))
    }
}
