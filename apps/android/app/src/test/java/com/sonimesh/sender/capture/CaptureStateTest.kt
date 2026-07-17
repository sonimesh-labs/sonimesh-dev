package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureStateTest {
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
