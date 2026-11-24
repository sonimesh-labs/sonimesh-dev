package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class CapturePeakTest {
    @Test
    fun peak_includes_negative_full_scale_and_only_completed_samples() {
        val samples = shortArrayOf(0, -1, 400, Short.MIN_VALUE)
        assertEquals(400, capturePeakMagnitude(samples, 3))
        assertEquals(32_768, capturePeakMagnitude(samples, 4))
        assertEquals(0, capturePeakMagnitude(shortArrayOf(0, 0), 2))
    }

    @Test
    fun evidence_retains_run_peak_and_clears_on_new_run() {
        val evidence = CaptureEvidence()
        evidence.begin(10)
        assertNull(evidence.snapshot(10).peakSampleMagnitude)
        evidence.recordRead(1, silent = true, completedAtMillis = 11, peakMagnitude = 0)
        assertEquals(0, evidence.snapshot(11).peakSampleMagnitude)
        evidence.recordRead(1, silent = false, completedAtMillis = 12, peakMagnitude = 100)
        evidence.recordRead(1, silent = false, completedAtMillis = 13, peakMagnitude = 50)
        assertEquals(100, evidence.snapshot(13).peakSampleMagnitude)
        evidence.begin(20)
        assertNull(evidence.snapshot(20).peakSampleMagnitude)
    }
}
