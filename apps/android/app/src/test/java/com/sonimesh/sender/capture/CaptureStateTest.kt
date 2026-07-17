package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureStateTest {
    @Test
    fun observed_format_is_bounded_and_derives_a_fixed_duration_read() {
        val format = CaptureFormat.fromObserved(
            sampleRateHz = 48_000,
            channels = 2,
            bufferSizeBytes = 7_680,
        )

        assertEquals(1_920, format?.readSamples)
    }

    @Test
    fun observed_format_rejects_unbounded_values() {
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES + 1,
            ),
        )
        assertEquals(
            null,
            CaptureFormat.fromObserved(
                sampleRateHz = MAX_CAPTURE_SAMPLE_RATE_HZ + 1,
                channels = 2,
                bufferSizeBytes = MAX_CAPTURE_BUFFER_BYTES,
            ),
        )
    }

    @Test
    fun evidence_counts_frames_silence_and_errors_without_audio_storage() {
        val evidence = CaptureEvidence()

        evidence.recordRead(frames = 480, silent = true)
        evidence.recordRead(frames = 240, silent = false)
        evidence.recordReadError()

        assertEquals(
            CaptureEvidenceSnapshot(
                capturedFrames = 720,
                silentReads = 1,
                readErrors = 1,
            ),
            evidence.snapshot(),
        )
    }

    @Test
    fun evidence_reset_discards_only_counters() {
        val evidence = CaptureEvidence()
        evidence.recordRead(frames = 480, silent = true)

        evidence.reset()

        assertEquals(CaptureEvidenceSnapshot(0, 0, 0), evidence.snapshot())
    }
}
