package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
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
}
