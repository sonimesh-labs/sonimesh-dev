package com.sonimesh.sender

import com.sonimesh.sender.capture.CaptureFormat
import com.sonimesh.sender.capture.CaptureStatus
import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureStatusDescriptionTest {
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
