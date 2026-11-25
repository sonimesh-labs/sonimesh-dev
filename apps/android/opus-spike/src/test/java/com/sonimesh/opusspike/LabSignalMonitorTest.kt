package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class LabSignalMonitorTest {
    @Test
    fun partialReadsCountFramesAndSignalResumesOnlyOnce() {
        val monitor = LabSignalMonitor()
        val samples = ShortArray(1_920)
        samples[0] = 10
        repeat(199) { assertNull(monitor.observe(samples, 960, 960)) }
        assertEquals(LabSignalTransition.NoCapturableAudio, monitor.observe(samples, 960, 960))
        assertNull(monitor.observe(samples, 960, 960))
        assertEquals(LabSignalTransition.SignalDetected, monitor.observe(samples, 0, 960))
        assertNull(monitor.observe(samples, 0, 960))
        repeat(199) { assertNull(monitor.observe(samples, 960, 960)) }
        assertEquals(LabSignalTransition.NoCapturableAudio, monitor.observe(samples, 960, 960))
    }

    @Test
    fun nonSilentReadResetsAnUnfinishedSilentInterval() {
        val monitor = LabSignalMonitor()
        val silence = ShortArray(1_920)
        val signal = ShortArray(1_920).apply { this[lastIndex] = -1 }
        repeat(99) { assertNull(monitor.observe(silence, 0, silence.size)) }
        assertEquals(LabSignalTransition.SignalDetected, monitor.observe(signal, 0, signal.size))
        assertNull(monitor.observe(signal, 0, signal.size))
        repeat(99) { assertNull(monitor.observe(silence, 0, silence.size)) }
        assertEquals(LabSignalTransition.NoCapturableAudio, monitor.observe(silence, 0, silence.size))
    }
}
