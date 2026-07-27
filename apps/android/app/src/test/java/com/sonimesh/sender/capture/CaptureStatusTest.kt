package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureStatusTest {
    @Test
    fun authorization_states_disable_a_second_start_request() {
        assertTrue(CaptureStatus.AwaitingPermission.isAwaitingAuthorization())
        assertTrue(CaptureStatus.AwaitingNotificationPermission.isAwaitingAuthorization())
        assertTrue(CaptureStatus.AwaitingProjectionConsent.isAwaitingAuthorization())
        assertFalse(CaptureStatus.Idle.isAwaitingAuthorization())
        assertFalse(CaptureStatus.Failed("permission rejected").isAwaitingAuthorization())
    }

    @Test
    fun only_running_capture_states_expose_the_stop_action() {
        assertTrue(CaptureStatus.Capturing(observedFormat()).isActive())
        assertTrue(CaptureStatus.NoCapturableAudio(observedFormat()).isActive())
        assertFalse(CaptureStatus.AwaitingProjectionConsent.isActive())
        assertFalse(CaptureStatus.Stopped(CaptureStopReason.UserRequested).isActive())
    }

    @Test
    fun terminal_and_idle_states_can_start_a_fresh_request() {
        assertTrue(CaptureStatus.Idle.canStart())
        assertTrue(CaptureStatus.Stopped(CaptureStopReason.UserRequested).canStart())
        assertTrue(CaptureStatus.Failed("permission rejected").canStart())
        assertFalse(CaptureStatus.AwaitingPermission.canStart())
        assertFalse(CaptureStatus.AwaitingNotificationPermission.canStart())
        assertFalse(CaptureStatus.Capturing(observedFormat()).canStart())
    }

    @Test
    fun retiring_service_rejection_only_replaces_the_matching_consent_state() {
        assertEquals(
            CaptureStatus.Failed(CAPTURE_SERVICE_RETIRING_MESSAGE),
            CaptureStatus.AwaitingProjectionConsent.retiringServiceStartFailure(),
        )
        assertEquals(null, CaptureStatus.AwaitingPermission.retiringServiceStartFailure())
        assertEquals(null, CaptureStatus.Capturing(observedFormat()).retiringServiceStartFailure())
        assertEquals(
            null,
            CaptureStatus.Stopped(CaptureStopReason.UserRequested).retiringServiceStartFailure(),
        )
    }

    private fun observedFormat(): CaptureFormat = requireNotNull(
        CaptureFormat.fromObserved(
            sampleRateHz = 48_000,
            channels = 2,
            audioSource = 8,
            channelMask = 0xC,
            bufferSizeBytes = 7_680,
            minimumBufferBytes = 3_840,
        ),
    )
}
