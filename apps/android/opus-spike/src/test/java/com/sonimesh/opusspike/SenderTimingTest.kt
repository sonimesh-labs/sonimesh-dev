package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Test

class SenderTimingTest {
    @Test
    fun counts_intervals_and_excludes_the_ten_millisecond_boundary() {
        val timing = SenderTiming()
        assertEquals(SenderTimingSnapshot(0, 0), timing.record(1_000_000))
        assertEquals(SenderTimingSnapshot(1, 1), timing.record(10_999_999))
        assertEquals(SenderTimingSnapshot(2, 1), timing.record(20_999_999))
        assertEquals(SenderTimingSnapshot(3, 1), timing.record(60_999_999))
        assertEquals(SenderTimingSnapshot(3, 1), timing.snapshot())
    }

    @Test
    fun a_clock_regression_restarts_the_interval() {
        val timing = SenderTiming()
        timing.record(20_000_000)
        assertEquals(SenderTimingSnapshot(0, 0), timing.record(10_000_000))
        assertEquals(SenderTimingSnapshot(1, 1), timing.record(11_000_000))
    }
}
