package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class CaptureReadTest {
    @Test
    fun successful_read_does_not_increment_the_error_counter() {
        val evidence = CaptureEvidence()

        val samplesRead = readCaptureSamples(evidence, shouldRecordFailure = { true }) { 960 }

        assertEquals(960, samplesRead)
        assertEquals(0, evidence.snapshot(nowMillis = 0).readErrors)
    }

    @Test
    fun thrown_read_failure_is_counted_once_and_rethrown() {
        for (failure in listOf(
            IllegalStateException("invalid record state"),
            IllegalArgumentException("invalid read bounds"),
            SecurityException("permission revoked"),
        )) {
            val evidence = CaptureEvidence()

            val thrown = assertThrows(failure.javaClass) {
                readCaptureSamples(evidence, shouldRecordFailure = { true }) { throw failure }
            }

            assertSame(failure, thrown)
            assertEquals(1, evidence.snapshot(nowMillis = 0).readErrors)
        }
    }

    @Test
    fun completed_frames_require_bounded_channel_aligned_samples() {
        assertEquals(480, completedCaptureFrames(960, 1_920, 2))
        assertEquals(960, completedCaptureFrames(1_920, 1_920, 2))

        for ((samplesRead, requestedSamples, channels) in listOf(
            Triple(-1, 1_920, 2),
            Triple(0, 1_920, 2),
            Triple(1, 1_920, 2),
            Triple(1_921, 1_920, 2),
            Triple(960, 0, 2),
            Triple(960, 1_919, 2),
            Triple(960, 1_920, 0),
        )) {
            assertEquals(null, completedCaptureFrames(samplesRead, requestedSamples, channels))
        }
    }

    @Test
    fun teardown_wakeup_is_rethrown_without_counting_a_read_error() {
        val evidence = CaptureEvidence()
        val failure = IllegalStateException("record stopped for teardown")

        val thrown = assertThrows(IllegalStateException::class.java) {
            readCaptureSamples(evidence, shouldRecordFailure = { false }) { throw failure }
        }

        assertSame(failure, thrown)
        assertEquals(0, evidence.snapshot(nowMillis = 0).readErrors)
    }
}
