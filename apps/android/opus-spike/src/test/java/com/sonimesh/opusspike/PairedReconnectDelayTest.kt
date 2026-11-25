package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Test

class PairedReconnectDelayTest {
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
