package com.sonimesh.sender

import com.sonimesh.sender.capture.CaptureEvidenceSnapshot
import com.sonimesh.sender.capture.CaptureFormat
import com.sonimesh.sender.capture.CaptureStatus
import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureStatusDescriptionTest {
    @Test
    fun signal_label_waits_for_non_silent_capture_and_honors_no_signal_status() {
        val format = requireNotNull(CaptureFormat.fromObserved(48_000, 2, 8, 12, 7_680, 3_840))
        val capturing = CaptureStatus.Capturing(format)
        assertEquals("Checking", capturing.signalLabel(CaptureEvidenceSnapshot()))
        assertEquals("Checking", capturing.signalLabel(CaptureEvidenceSnapshot(capturedFrames = 960, silentFrames = 960)))
        val signal = CaptureEvidenceSnapshot(capturedFrames = 1_920, silentFrames = 960)
        assertEquals("Observed", capturing.signalLabel(signal))
        assertEquals("None", CaptureStatus.NoCapturableAudio(format).signalLabel(signal))
        assertEquals("Waiting", CaptureStatus.Idle.signalLabel(signal))
    }

    @Test
    fun evidence_description_reports_silent_frames_separately_from_read_count() {
        val evidence = CaptureEvidenceSnapshot(
            captureDurationMillis = 2_500,
            lastReadCompletionOffsetMillis = 2_480,
            capturedFrames = 120_000,
            silentFrames = 96_000,
            silentReads = 150,
            readErrors = 1,
        )

        assertEquals(
            "Observed 120000 frames over 2.5 s, 96000 silent frames across 150 reads, " +
                "1 read errors. Last successful read at +2.4 s.",
            evidence.description(),
        )
    }

    @Test
    fun no_capturable_audio_keeps_the_observed_format_visible() {
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

        assertEquals(
            "No capturable audio detected. 48000 Hz, 2 channels, source 8, channel mask 0xc, 7680 byte buffer (platform minimum 3840). The source may be silent or policy-restricted; the cause is not identifiable here.",
            CaptureStatus.NoCapturableAudio(format).description(),
        )
    }
}
