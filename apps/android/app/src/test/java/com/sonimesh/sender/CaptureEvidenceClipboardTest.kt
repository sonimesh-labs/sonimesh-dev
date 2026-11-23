package com.sonimesh.sender

import com.sonimesh.sender.capture.CaptureEvidenceSnapshot
import com.sonimesh.sender.capture.CaptureFormat
import com.sonimesh.sender.capture.CaptureSource
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.CaptureStopReason
import com.sonimesh.sender.capture.CaptureUiState
import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureEvidenceClipboardTest {
    @Test
    fun share_content_contains_only_the_text_evidence_summary() {
        val content = captureEvidenceShareContent("SoniMesh capture evidence")

        assertEquals("text/plain", content.mimeType)
        assertEquals("SoniMesh capture evidence", content.text)
    }

    @Test
    fun copyable_evidence_contains_status_format_and_observations() {
        val format = requireNotNull(
            CaptureFormat.fromObserved(
                sampleRateHz = 48_000,
                channels = 2,
                audioSource = 8,
                channelMask = 0xC,
                bufferSizeBytes = 7_680,
                minimumBufferBytes = 3_840,
            ),
        )
        val state = CaptureUiState(
            status = CaptureStatus.NoCapturableAudio(format),
            evidence = CaptureEvidenceSnapshot(
                captureDurationMillis = 2_500,
                captureSource = CaptureSource.Game,
                lastReadCompletionOffsetMillis = 2_480,
                capturedFrames = 120_000,
                silentFrames = 96_000,
                silentReads = 150,
                readErrors = 1,
            ),
        )

        assertEquals(
            "SoniMesh capture evidence\n" +
                "Status: No capturable audio detected. 48000 Hz, 2 channels, source 8, " +
                "channel mask 0xc, 7680 byte buffer (platform minimum 3840). The source " +
                "may be silent or policy-restricted; the cause is not identifiable here.\n" +
                "Observed 120000 frames over 2.5 s, 96000 silent frames across 150 reads, " +
                "1 read errors. Requested usage: Game. Last successful read at +2.4 s.",
            state.copyableEvidence(),
        )
    }

    @Test
    fun stopped_and_failed_capture_evidence_include_retained_format() {
        val format = requireNotNull(CaptureFormat.fromObserved(48_000, 2, 8, 0xC, 7_680, 3_840))
        val evidence = CaptureEvidenceSnapshot(captureFormat = format)
        for (status in listOf(CaptureStatus.Stopped(CaptureStopReason.UserRequested), CaptureStatus.Failed("Read failed"))) {
            val text = CaptureUiState(status = status, evidence = evidence).copyableEvidence()
            org.junit.Assert.assertTrue(text.contains("Capture format: 48000 Hz, 2 channels, source 8, channel mask 0xc, 7680 byte buffer (platform minimum 3840)."))
            org.junit.Assert.assertTrue(evidence.hasObservations())
        }
    }
}
