package com.sonimesh.sender.capture

import android.os.SystemClock
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

class CaptureEvidence {
    private val captureStartedAtMillis = AtomicLong(NO_CAPTURE_START)
    private val lastReadCompletedAtMillis = AtomicLong(NO_READ_COMPLETION)
    private val capturedFrames = AtomicLong()
    private val silentReads = AtomicLong()
    private val readErrors = AtomicLong()

    /** Records one successful read and its local monotonic completion time. */
    fun recordRead(frames: Int, silent: Boolean, completedAtMillis: Long) {
        require(completedAtMillis >= 0) { "read completion time must be monotonic" }
        capturedFrames.addAndGet(frames.toLong())
        if (silent) silentReads.incrementAndGet()
        val startedAtMillis = captureStartedAtMillis.get()
        if (startedAtMillis != NO_CAPTURE_START && completedAtMillis >= startedAtMillis) {
            recordLatestReadCompletion(completedAtMillis)
        }
    }

    fun recordReadError() {
        readErrors.incrementAndGet()
    }

    /** Starts a fresh evidence interval at the supplied monotonic timestamp. */
    fun begin(captureStartedAtMillis: Long) {
        require(captureStartedAtMillis >= 0) { "capture start time must be monotonic" }
        reset()
        this.captureStartedAtMillis.set(captureStartedAtMillis)
    }

    fun reset() {
        captureStartedAtMillis.set(NO_CAPTURE_START)
        lastReadCompletedAtMillis.set(NO_READ_COMPLETION)
        capturedFrames.set(0)
        silentReads.set(0)
        readErrors.set(0)
    }

    /** Returns a low-rate summary at the supplied monotonic timestamp. */
    fun snapshot(nowMillis: Long): CaptureEvidenceSnapshot {
        val startedAtMillis = captureStartedAtMillis.get()
        val captureDurationMillis = if (startedAtMillis == NO_CAPTURE_START) {
            0
        } else {
            (nowMillis - startedAtMillis).coerceAtLeast(0)
        }
        val lastReadCompletionOffsetMillis = lastReadCompletedAtMillis.get()
            .takeIf { completedAtMillis ->
                startedAtMillis != NO_CAPTURE_START && completedAtMillis >= startedAtMillis
            }
            ?.minus(startedAtMillis)
        return CaptureEvidenceSnapshot(
            captureDurationMillis = captureDurationMillis,
            lastReadCompletionOffsetMillis = lastReadCompletionOffsetMillis,
            capturedFrames = capturedFrames.get(),
            silentReads = silentReads.get(),
            readErrors = readErrors.get(),
        )
    }

    private fun recordLatestReadCompletion(completedAtMillis: Long) {
        while (true) {
            val previous = lastReadCompletedAtMillis.get()
            if (completedAtMillis <= previous
                || lastReadCompletedAtMillis.compareAndSet(previous, completedAtMillis)
            ) {
                return
            }
        }
    }

    private companion object {
        const val NO_CAPTURE_START = -1L
        const val NO_READ_COMPLETION = -1L
    }
}

object CaptureStateStore {
    private val mutableState = MutableStateFlow(CaptureUiState())

    val state: StateFlow<CaptureUiState> = mutableState.asStateFlow()
    val evidence = CaptureEvidence()

    fun publishAwaitingPermission() {
        evidence.reset()
        publish(CaptureStatus.AwaitingPermission)
    }

    fun publishAwaitingNotificationPermission() {
        publish(CaptureStatus.AwaitingNotificationPermission)
    }

    fun publishAwaitingProjectionConsent() {
        publish(CaptureStatus.AwaitingProjectionConsent)
    }

    fun publishCapturing(format: CaptureFormat) {
        publish(CaptureStatus.Capturing(format))
    }

    fun publishNoCapturableAudio(format: CaptureFormat) {
        publish(CaptureStatus.NoCapturableAudio(format))
    }

    fun publishStopped(reason: CaptureStopReason) {
        publish(CaptureStatus.Stopped(reason))
    }

    fun publishFailure(message: String) {
        publish(CaptureStatus.Failed(message))
    }

    /** Publishes the latest bounded evidence only while capture remains active. */
    fun publishEvidence() {
        val current = mutableState.value
        if (current.status !is CaptureStatus.Capturing
            && current.status !is CaptureStatus.NoCapturableAudio
        ) {
            return
        }
        mutableState.value = current.copy(evidence = evidence.snapshot(SystemClock.elapsedRealtime()))
    }

    private fun publish(status: CaptureStatus) {
        mutableState.value = CaptureUiState(
            status = status,
            evidence = evidence.snapshot(SystemClock.elapsedRealtime()),
        )
    }
}
