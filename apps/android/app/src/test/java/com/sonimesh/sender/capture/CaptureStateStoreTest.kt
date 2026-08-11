package com.sonimesh.sender.capture

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Before
import org.junit.Test

class CaptureStateStoreTest {
    @Before
    fun resetStore() {
        CaptureStateStore.nowMillis = { 0L }
        CaptureStateStore.publishAwaitingPermission()
    }

    @After
    fun restoreClock() {
        CaptureStateStore.nowMillis = android.os.SystemClock::elapsedRealtime
    }

    @Test
    fun capturing_publish_attaches_the_latest_bounded_evidence() {
        CaptureStateStore.evidence.begin(captureStartedAtMillis = 0)
        CaptureStateStore.evidence.recordRead(
            frames = 960,
            silent = true,
            completedAtMillis = 10,
        )
        CaptureStateStore.nowMillis = { 100L }

        CaptureStateStore.publishCapturing(format())

        val state = CaptureStateStore.state.value
        assertEquals(CaptureStatus.Capturing(format()), state.status)
        assertEquals(100L, state.evidence.captureDurationMillis)
        assertEquals(10L, state.evidence.lastReadCompletionOffsetMillis)
        assertEquals(960L, state.evidence.capturedFrames)
        assertEquals(960L, state.evidence.silentFrames)
        assertEquals(1L, state.evidence.silentReads)
    }

    @Test
    fun publish_evidence_is_a_no_op_outside_active_capture() {
        CaptureStateStore.evidence.begin(captureStartedAtMillis = 0)
        CaptureStateStore.evidence.recordRead(
            frames = 960,
            silent = false,
            completedAtMillis = 10,
        )
        CaptureStateStore.nowMillis = { 100L }
        CaptureStateStore.publishStopped(CaptureStopReason.UserRequested)
        val before = CaptureStateStore.state.value.evidence

        CaptureStateStore.evidence.recordRead(
            frames = 480,
            silent = true,
            completedAtMillis = 20,
        )
        CaptureStateStore.nowMillis = { 200L }
        CaptureStateStore.publishEvidence()

        val state = CaptureStateStore.state.value
        assertEquals(CaptureStatus.Stopped(CaptureStopReason.UserRequested), state.status)
        assertEquals(before, state.evidence)
    }

    @Test
    fun publish_evidence_updates_while_no_capturable_audio_remains_active() {
        CaptureStateStore.evidence.begin(captureStartedAtMillis = 50)
        CaptureStateStore.evidence.recordRead(
            frames = 960,
            silent = true,
            completedAtMillis = 90,
        )
        CaptureStateStore.nowMillis = { 150L }
        CaptureStateStore.publishNoCapturableAudio(format())
        val before = CaptureStateStore.state.value.evidence

        CaptureStateStore.evidence.recordRead(
            frames = 480,
            silent = false,
            completedAtMillis = 100,
        )
        CaptureStateStore.nowMillis = { 200L }
        CaptureStateStore.publishEvidence()

        val state = CaptureStateStore.state.value
        assertEquals(CaptureStatus.NoCapturableAudio(format()), state.status)
        assertEquals(150L, state.evidence.captureDurationMillis)
        assertEquals(1_440L, state.evidence.capturedFrames)
        assertEquals(960L, state.evidence.silentFrames)
        assertNotEquals(before, state.evidence)
    }

    @Test
    fun retiring_start_failure_replaces_only_fresh_projection_consent() {
        CaptureStateStore.publishAwaitingProjectionConsent()

        CaptureStateStore.publishRetiringServiceStartFailure()

        assertEquals(
            CaptureStatus.Failed(CAPTURE_SERVICE_RETIRING_MESSAGE),
            CaptureStateStore.state.value.status,
        )
    }

    @Test
    fun retiring_start_failure_leaves_other_states_untouched() {
        CaptureStateStore.publishAwaitingNotificationPermission()

        CaptureStateStore.publishRetiringServiceStartFailure()

        assertEquals(
            CaptureStatus.AwaitingNotificationPermission,
            CaptureStateStore.state.value.status,
        )
    }

    @Test
    fun awaiting_permission_resets_evidence_before_the_next_session() {
        CaptureStateStore.evidence.begin(captureStartedAtMillis = 0)
        CaptureStateStore.evidence.recordRead(
            frames = 960,
            silent = true,
            completedAtMillis = 10,
        )
        CaptureStateStore.publishCapturing(format())

        CaptureStateStore.publishAwaitingPermission()

        val state = CaptureStateStore.state.value
        assertEquals(CaptureStatus.AwaitingPermission, state.status)
        assertEquals(0L, state.evidence.capturedFrames)
        assertEquals(0L, state.evidence.captureDurationMillis)
        assertEquals(null, state.evidence.lastReadCompletionOffsetMillis)
    }

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
