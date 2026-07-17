package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class CaptureStateTest {
    @Test
    fun observed_format_is_bounded_and_derives_a_fixed_duration_read() {
        val format = CaptureFormat.fromObserved(
            sampleRateHz = 48_000,
            channels = 2,
            bufferSizeBytes = 7_680,
            minimumBufferBytes = 3_840,
        )

        assertEquals(1_920, format?.readSamples)
        assertEquals(3_840, format?.minimumBufferBytes)
    }

    @Test
    fun observed_format_rejects_unbounded_values() {
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES + 1,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = MAX_CAPTURE_SAMPLE_RATE_HZ + 1,
                channels = 2,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES,
                minimumBufferBytes = MAX_CAPTURE_BUFFER_BYTES + 1,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                bufferSizeBytes = 3_840,
                minimumBufferBytes = 7_680,
            ),
        )
    }

    @Test
    fun evidence_counts_frames_silence_and_errors_without_audio_storage() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000)

        evidence.recordRead(frames = 480, silent = true, completedAtMillis = 1_020)
        evidence.recordRead(frames = 240, silent = false, completedAtMillis = 1_030)
        evidence.recordReadError()

        assertEquals(
            CaptureEvidenceSnapshot(
                captureDurationMillis = 750,
                lastReadCompletionOffsetMillis = 30,
                capturedFrames = 720,
                silentReads = 1,
                readErrors = 1,
            ),
            evidence.snapshot(nowMillis = 1_750),
        )
    }

    @Test
    fun evidence_reset_discards_only_counters() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000)
        evidence.recordRead(frames = 480, silent = true, completedAtMillis = 1_020)

        evidence.reset()

        assertEquals(CaptureEvidenceSnapshot(), evidence.snapshot(nowMillis = 1_500))
    }

    @Test
    fun evidence_duration_stays_safe_when_a_snapshot_clock_is_earlier_than_start() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000)

        assertEquals(0, evidence.snapshot(nowMillis = 999).captureDurationMillis)
    }

    @Test
    fun evidence_keeps_the_latest_valid_read_completion_offset() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000)

        evidence.recordRead(frames = 480, silent = false, completedAtMillis = 1_020)
        evidence.recordRead(frames = 480, silent = false, completedAtMillis = 1_010)
        evidence.recordRead(frames = 480, silent = false, completedAtMillis = 999)

        assertEquals(20L, evidence.snapshot(nowMillis = 1_100).lastReadCompletionOffsetMillis)
    }

    @Test
    fun evidence_rejects_a_negative_read_completion_time() {
        val evidence = CaptureEvidence()

        try {
            evidence.recordRead(frames = 480, silent = false, completedAtMillis = -1)
            fail("expected a negative read completion time to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: capture evidence uses the local monotonic clock only.
        }
    }

    @Test
    fun evidence_rejects_a_negative_capture_start_time() {
        val evidence = CaptureEvidence()

        try {
            evidence.begin(captureStartedAtMillis = -1)
            fail("expected a negative capture start time to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: duration calculations require a monotonic timestamp.
        }
    }

    @Test
    fun signal_monitor_reports_only_one_transition_after_the_silence_limit() {
        val monitor = CaptureSignalMonitor(silentReadLimit = 3)

        assertNull(monitor.observe(silent = true))
        assertNull(monitor.observe(silent = true))
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(silent = true))
        assertNull(monitor.observe(silent = true))
    }

    @Test
    fun signal_monitor_reports_recovery_once_after_non_silent_audio_returns() {
        val monitor = CaptureSignalMonitor(silentReadLimit = 2)

        monitor.observe(silent = true)
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(silent = true))
        assertEquals(CaptureSignalTransition.SignalDetected, monitor.observe(silent = false))
        assertNull(monitor.observe(silent = false))
        assertNull(monitor.observe(silent = true))
        assertEquals(CaptureSignalTransition.NoCapturableAudio, monitor.observe(silent = true))
    }

    @Test
    fun signal_monitor_rejects_a_non_positive_silence_threshold() {
        try {
            CaptureSignalMonitor(silentReadLimit = 0)
            fail("expected an invalid silence threshold to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: a zero threshold would publish a state without a read.
        }
    }

    @Test
    fun evidence_publish_throttle_allows_at_most_one_snapshot_per_interval() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 500)

        assertTrue(throttle.shouldPublish(nowMillis = 0))
        assertFalse(throttle.shouldPublish(nowMillis = 499))
        assertTrue(throttle.shouldPublish(nowMillis = 500))
        assertFalse(throttle.shouldPublish(nowMillis = 999))
        assertTrue(throttle.shouldPublish(nowMillis = 1_000))
    }

    @Test
    fun evidence_publish_throttle_rejects_a_non_positive_interval() {
        try {
            CaptureEvidencePublishThrottle(minimumIntervalMillis = 0)
            fail("expected an invalid evidence publication interval to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: a zero interval would allow per-read UI publication.
        }
    }
}
