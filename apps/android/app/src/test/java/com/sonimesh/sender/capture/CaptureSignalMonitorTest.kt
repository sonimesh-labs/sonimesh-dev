package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

class CaptureSignalMonitorTest {
    @Test
    fun constructor_rejects_non_positive_thresholds() {
        assertThrows(IllegalArgumentException::class.java) { CaptureSignalMonitor(0) }
        assertThrows(IllegalArgumentException::class.java) { CaptureSignalMonitor(-1) }
    }

    @Test
    fun observe_rejects_incomplete_reads() {
        val monitor = CaptureSignalMonitor(10)

        assertThrows(IllegalArgumentException::class.java) { monitor.observe(true, 0) }
    }

    @Test
    fun silence_accumulates_until_threshold_then_reports_once() {
        val monitor = CaptureSignalMonitor(10)

        assertNull(monitor.observe(true, 4))
        assertNull(monitor.observe(true, 3))
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(true, 3))

        // Already reported; further silence stays quiet until a signal returns.
        assertNull(monitor.observe(true, 10))
    }

    @Test
    fun single_large_silent_read_clamps_to_threshold() {
        val monitor = CaptureSignalMonitor(10)

        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(true, 25))
        assertNull(monitor.observe(true, 1))
    }

    @Test
    fun detected_signal_clears_report_before_next_silent_run() {
        val monitor = CaptureSignalMonitor(10)

        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(true, 10))
        assertEquals(CaptureSignalTransition.SignalDetected, monitor.observe(false, 1))
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(true, 10))
    }

    @Test
    fun non_silent_reads_reset_the_silent_run_without_reporting() {
        val monitor = CaptureSignalMonitor(10)

        assertNull(monitor.observe(true, 6))
        assertNull(monitor.observe(false, 1))
        assertNull(monitor.observe(true, 6))
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(true, 4))
    }
}
