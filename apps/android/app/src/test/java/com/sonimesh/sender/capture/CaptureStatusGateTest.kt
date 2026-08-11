package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureStatusGateTest {
    private val capturing = CaptureStatus.Capturing(format())
    private val noCapturableAudio = CaptureStatus.NoCapturableAudio(format())
    private val stopped = CaptureStatus.Stopped(CaptureStopReason.UserRequested)
    private val failed = CaptureStatus.Failed("boot failure")

    @Test
    fun is_active_only_while_the_service_owns_live_capture() {
        assertTrue(capturing.isActive())
        assertTrue(noCapturableAudio.isActive())

        for (status in inactiveStatuses()) {
            assertFalse(status.isActive())
        }
    }

    @Test
    fun is_awaiting_authorization_only_for_open_system_prompt_paths() {
        for (status in listOf(
            CaptureStatus.AwaitingPermission,
            CaptureStatus.AwaitingNotificationPermission,
            CaptureStatus.AwaitingProjectionConsent,
        )) {
            assertTrue(status.isAwaitingAuthorization())
        }

        for (status in listOf(
            CaptureStatus.Idle,
            capturing,
            noCapturableAudio,
            stopped,
            failed,
        )) {
            assertFalse(status.isAwaitingAuthorization())
        }
    }

    @Test
    fun can_start_only_when_inactive_and_not_awaiting_authorization() {
        for (status in listOf(CaptureStatus.Idle, stopped, failed)) {
            assertTrue(status.canStart())
        }

        for (status in listOf(
            CaptureStatus.AwaitingPermission,
            CaptureStatus.AwaitingNotificationPermission,
            CaptureStatus.AwaitingProjectionConsent,
            capturing,
            noCapturableAudio,
        )) {
            assertFalse(status.canStart())
        }
    }

    @Test
    fun retiring_failure_only_replaces_fresh_projection_consent() {
        assertEquals(
            CaptureStatus.Failed(CAPTURE_SERVICE_RETIRING_MESSAGE),
            CaptureStatus.AwaitingProjectionConsent.retiringServiceStartFailure(),
        )

        for (status in listOf(
            CaptureStatus.Idle,
            CaptureStatus.AwaitingPermission,
            CaptureStatus.AwaitingNotificationPermission,
            capturing,
            noCapturableAudio,
            stopped,
            failed,
        )) {
            assertNull(status.retiringServiceStartFailure())
        }
    }

    @Test
    fun retiring_message_is_the_actionable_retry_copy() {
        assertEquals(
            "Previous capture session is still stopping; wait briefly and start capture again",
            CAPTURE_SERVICE_RETIRING_MESSAGE,
        )
    }

    private fun inactiveStatuses(): List<CaptureStatus> = listOf(
        CaptureStatus.Idle,
        CaptureStatus.AwaitingPermission,
        CaptureStatus.AwaitingNotificationPermission,
        CaptureStatus.AwaitingProjectionConsent,
        stopped,
        failed,
    )

    private fun format(): CaptureFormat = CaptureFormat(
        sampleRateHz = 48_000,
        channels = 2,
        audioSource = 1,
        channelMask = 12,
        bufferSizeBytes = 16_384,
        minimumBufferBytes = 4_096,
        readSamples = 1_920,
    )
}
