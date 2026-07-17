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
        mutableState.value = CaptureUiState(CaptureStatus.AwaitingPermission)
    }

    fun publishAwaitingProjectionConsent() {
        mutableState.value = CaptureUiState(CaptureStatus.AwaitingProjectionConsent)
    }

    fun publishCapturing(format: CaptureFormat) {
        mutableState.value = CaptureUiState(CaptureStatus.Capturing(format))
    }

    fun publishNoCapturableAudio(format: CaptureFormat) {
        mutableState.value = CaptureUiState(CaptureStatus.NoCapturableAudio(format))
    }

    fun publishStopped(reason: CaptureStopReason) {
        mutableState.value = CaptureUiState(CaptureStatus.Stopped(reason))
    }

    fun publishFailure(message: String) {
        mutableState.value = CaptureUiState(CaptureStatus.Failed(message))
    }
}
