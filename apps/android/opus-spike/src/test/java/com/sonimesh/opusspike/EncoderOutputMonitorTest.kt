package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class EncoderOutputMonitorTest {
    @Test fun warnsOnlyAfterSustainedAcceptedSignalAndRecoversOnPacket() {
        val monitor = EncoderOutputMonitor()
        assertNull(monitor.accepted(0, true))
        assertNull(monitor.accepted(1_999, true))
        assertEquals(true, monitor.accepted(2_000, true))
        assertNull(monitor.accepted(2_100, true))
        assertEquals(false, monitor.packet())
        assertNull(monitor.accepted(2_200, true))
    }

    @Test fun silentOrMutedInputClearsAndRestartsObservation() {
        val monitor = EncoderOutputMonitor()
        assertNull(monitor.accepted(0, false))
        assertNull(monitor.accepted(3_000, true))
        assertNull(monitor.accepted(4_999, false))
        assertNull(monitor.accepted(5_000, true))
        assertEquals(true, monitor.accepted(7_000, true))
        assertEquals(false, monitor.accepted(7_001, false))
        assertNull(monitor.accepted(10_000, false))
    }

    @Test fun packetAndClockResetRestartTheInterval() {
        val monitor = EncoderOutputMonitor()
        assertNull(monitor.accepted(5_000, true))
        assertNull(monitor.packet())
        assertNull(monitor.accepted(6_000, true))
        assertNull(monitor.accepted(4_000, true))
        assertNull(monitor.accepted(5_999, true))
        assertEquals(true, monitor.accepted(6_000, true))
        assertEquals(false, monitor.accepted(1_000, true))
    }
}
