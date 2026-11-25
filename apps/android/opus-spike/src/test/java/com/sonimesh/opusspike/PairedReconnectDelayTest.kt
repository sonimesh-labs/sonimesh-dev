package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairedReconnectDelayTest {
    @Test
    fun establishedSessionsResetConsecutiveReconnectFailures() {
        val failures = PairedReconnectFailures()
        repeat(20) {
            failures.sessionEnded(established = true)
            assertEquals(0, failures.count)
            assertEquals(500L, pairedReconnectDelayMillis(failures.count))
            repeat(3) { failures.sessionEnded(established = false) }
            assertEquals(3, failures.count)
        }
        failures.sessionEnded(established = true)
        repeat(12) { attempt ->
            failures.sessionEnded(established = false)
            assertEquals(attempt + 1, failures.count)
        }
        assertEquals(12, failures.count)
    }

    @Test
    fun reconnect_delay_grows_quickly_then_stays_bounded() {
        assertEquals(500L, pairedReconnectDelayMillis(0))
        assertEquals(1_000L, pairedReconnectDelayMillis(1))
        assertEquals(2_000L, pairedReconnectDelayMillis(2))
        assertEquals(4_000L, pairedReconnectDelayMillis(3))
        assertEquals(4_000L, pairedReconnectDelayMillis(20))
        assertEquals(500L, pairedReconnectDelayMillis(-1))
    }

    @Test
    fun reconnectWaitStopsAtDeadlineWithoutNegativeSleep() {
        val times = ArrayDeque(listOf(1_000L, 1_499L, 1_501L))
        val sleeps = mutableListOf<Long>()
        assertTrue(waitForPairedReconnect(500, { true }, { times.removeFirst() }, sleeps::add))
        assertEquals(listOf(1L), sleeps)
    }

    @Test
    fun reconnectWaitChecksStopBetweenBoundedSleeps() {
        var time = 0L
        var running = true
        val sleeps = mutableListOf<Long>()
        assertFalse(waitForPairedReconnect(500, { running }, { time }) {
            sleeps.add(it)
            time += it
            if (time == 200L) running = false
        })
        assertEquals(listOf(100L, 100L), sleeps)
    }

    @Test
    fun reconnectWaitPreservesInterrupt() {
        try {
            assertFalse(waitForPairedReconnect(500, { true }, { 0L }) { throw InterruptedException() })
            assertTrue(Thread.currentThread().isInterrupted)
        } finally {
            Thread.interrupted()
        }
    }
}
