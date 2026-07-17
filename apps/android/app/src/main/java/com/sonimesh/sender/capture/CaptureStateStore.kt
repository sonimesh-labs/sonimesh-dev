package com.sonimesh.sender.capture

import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

class CaptureEvidence {
    private val capturedFrames = AtomicLong()
    private val silentReads = AtomicLong()
    private val readErrors = AtomicLong()

    fun recordRead(frames: Int, silent: Boolean) {
        capturedFrames.addAndGet(frames.toLong())
        if (silent) silentReads.incrementAndGet()
    }

    fun recordReadError() {
        readErrors.incrementAndGet()
    }

    fun reset() {
        capturedFrames.set(0)
        silentReads.set(0)
        readErrors.set(0)
    }

    fun snapshot(): CaptureEvidenceSnapshot = CaptureEvidenceSnapshot(
        capturedFrames = capturedFrames.get(),
        silentReads = silentReads.get(),
        readErrors = readErrors.get(),
    )
}

object CaptureStateStore {
    private val mutableState = MutableStateFlow(CaptureUiState())

    val state: StateFlow<CaptureUiState> = mutableState.asStateFlow()
    val evidence = CaptureEvidence()

    fun publishAwaitingPermission() {
        evidence.reset()
        publish(CaptureStatus.AwaitingPermission)
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
        mutableState.value = current.copy(evidence = evidence.snapshot())
    }

    private fun publish(status: CaptureStatus) {
        mutableState.value = CaptureUiState(status = status, evidence = evidence.snapshot())
    }
}
