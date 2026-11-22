package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class CaptureStateTest {
    @Test
    fun observed_format_is_bounded_and_derives_a_fixed_duration_read() {
        val format = CaptureFormat.fromObserved(
            sampleRateHz = 48_000,
            channels = 2,
            audioSource = 8,
            channelMask = 0xC,
            bufferSizeBytes = 7_680,
            minimumBufferBytes = 3_840,
        )

        assertEquals(1_920, format?.readSamples)
        assertEquals(3_840, format?.minimumBufferBytes)
        assertEquals(8, format?.audioSource)
        assertEquals(0xC, format?.channelMask)
    }

    @Test
    fun observed_format_rejects_unbounded_values() {
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = 8,
                channelMask = 0xC,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES + 1,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = -1,
                channelMask = 0xC,
                bufferSizeBytes = 7_680,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = 8,
                channelMask = -1,
                bufferSizeBytes = 7_680,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = MAX_CAPTURE_SAMPLE_RATE_HZ + 1,
                channels = 2,
                audioSource = 8,
                channelMask = 0xC,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES,
                minimumBufferBytes = 3_840,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = 8,
                channelMask = 0xC,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES,
                minimumBufferBytes = MAX_CAPTURE_BUFFER_BYTES + 1,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = 8,
                channelMask = 0xC,
                bufferSizeBytes = 3_840,
                minimumBufferBytes = 7_680,
            ),
        )
    }

    @Test
    fun evidence_counts_frames_silence_and_errors_without_audio_storage() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000, captureSource = CaptureSource.Game)

        evidence.recordRead(frames = 480, silent = true, completedAtMillis = 1_020)
        evidence.recordRead(frames = 240, silent = false, completedAtMillis = 1_030)
        evidence.recordReadError()

        assertEquals(
            CaptureEvidenceSnapshot(
                captureDurationMillis = 750,
                captureSource = CaptureSource.Game,
                lastReadCompletionOffsetMillis = 30,
                capturedFrames = 720,
                silentFrames = 480,
                silentReads = 1,
                readErrors = 1,
            ),
            evidence.snapshot(nowMillis = 1_750),
        )
    }

    @Test
    fun evidence_reset_discards_only_counters() {
        val evidence = CaptureEvidence()
        evidence.begin(captureStartedAtMillis = 1_000, captureSource = CaptureSource.Unknown)
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
    fun evidence_rejects_a_negative_snapshot_time() {
        val evidence = CaptureEvidence()

        try {
            evidence.snapshot(nowMillis = -1)
            fail("expected a negative snapshot time to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: duration calculations require a monotonic timestamp.
        }
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
    fun evidence_rejects_a_non_positive_completed_frame_count() {
        val evidence = CaptureEvidence()

        for (frames in listOf(0, -1)) {
            try {
                evidence.recordRead(frames = frames, silent = true, completedAtMillis = 1_000)
                fail("expected a non-positive completed frame count to be rejected")
            } catch (_: IllegalArgumentException) {
                // Expected: empty or invalid reads are not capture evidence.
            }
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
    fun signal_monitor_reports_only_one_transition_after_the_silent_frame_threshold() {
        val monitor = CaptureSignalMonitor(silentFrameThreshold = 1_440)

        assertNull(monitor.observe(silent = true, frames = 480))
        assertNull(monitor.observe(silent = true, frames = 240))
        assertEquals(
            CaptureSignalTransition.NoCapturableAudio,
            monitor.observe(silent = true, frames = 800),
        )
        assertNull(monitor.observe(silent = true, frames = 960))
    }

    @Test
    fun partial_reads_cannot_trigger_no_signal_before_the_frame_threshold() {
        val monitor = CaptureSignalMonitor(silentFrameThreshold = 960)

        repeat(95) {
            assertNull(monitor.observe(silent = true, frames = 10))
        }
        assertNull(monitor.observe(silent = true, frames = 9))
        assertEquals(
            CaptureSignalTransition.NoCapturableAudio,
            monitor.observe(silent = true, frames = 1),
        )
    }

    @Test
    fun signal_monitor_reports_recovery_once_after_non_silent_audio_returns() {
        val monitor = CaptureSignalMonitor(silentFrameThreshold = 960)

        monitor.observe(silent = true, frames = 480)
        assertEquals(
            CaptureSignalTransition.NoCapturableAudio,
            monitor.observe(silent = true, frames = 480),
        )
        assertEquals(
            CaptureSignalTransition.SignalDetected,
            monitor.observe(silent = false, frames = 120),
        )
        assertNull(monitor.observe(silent = false, frames = 480))
        assertNull(monitor.observe(silent = true, frames = 959))
        assertEquals(
            CaptureSignalTransition.NoCapturableAudio,
            monitor.observe(silent = true, frames = 1),
        )
    }

    @Test
    fun signal_monitor_rejects_invalid_thresholds_and_empty_reads() {
        try {
            CaptureSignalMonitor(silentFrameThreshold = 0)
            fail("expected an invalid silence threshold to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: a zero threshold would publish a state without a frame.
        }

        val monitor = CaptureSignalMonitor(silentFrameThreshold = 960)
        try {
            monitor.observe(silent = true, frames = 0)
            fail("expected an empty completed read to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: zero-length non-blocking reads are filtered by the service.
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
    fun evidence_publish_throttle_rebases_after_a_nonnegative_clock_regression() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 500)

        assertTrue(throttle.shouldPublish(nowMillis = 1_000))
        assertFalse(throttle.shouldPublish(nowMillis = 1_200))
        assertTrue(throttle.shouldPublish(nowMillis = 999))
        assertFalse(throttle.shouldPublish(nowMillis = 1_498))
        assertTrue(throttle.shouldPublish(nowMillis = 1_499))
    }

    @Test
    fun evidence_publish_throttle_rejects_a_negative_timestamp() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 500)

        try {
            throttle.shouldPublish(nowMillis = -1)
            fail("expected a negative evidence publication time to be rejected")
        } catch (_: IllegalArgumentException) {
            // Expected: evidence publication uses the local monotonic clock only.
        }
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

    @Test
    fun failed_status_rejects_details_over_the_fixed_byte_bound() {
        assertThrows(IllegalArgumentException::class.java) {
            CaptureStatus.Failed("x".repeat(MAX_CAPTURE_ERROR_DETAIL_BYTES + 1))
        }
    }

    @Test
    fun failure_detail_is_truncated_without_splitting_utf8_code_points() {
        val message = "音".repeat(MAX_CAPTURE_ERROR_DETAIL_BYTES)

        val bounded = boundedCaptureErrorDetail(message)

        assertTrue(bounded.toByteArray(Charsets.UTF_8).size <= MAX_CAPTURE_ERROR_DETAIL_BYTES)
        assertTrue(bounded.endsWith("…"))
        val expectedPrefixBytes = (MAX_CAPTURE_ERROR_DETAIL_BYTES - "…".toByteArray(Charsets.UTF_8).size) / 3 * 3
        assertEquals(expectedPrefixBytes, bounded.removeSuffix("…").toByteArray(Charsets.UTF_8).size)
    }

    @Test
    fun short_failure_detail_is_preserved() {
        val message = "capture failed"

        assertEquals(message, boundedCaptureErrorDetail(message))
    }
}
