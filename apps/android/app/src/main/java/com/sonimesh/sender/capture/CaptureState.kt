package com.sonimesh.sender.capture

data class CaptureFormat(
    val sampleRateHz: Int,
    val channels: Int,
    val bufferSizeBytes: Int,
)

enum class CaptureStopReason(val displayName: String) {
    UserRequested("user requested"),
    ProjectionRevoked("projection revoked"),
    ServiceDestroyed("service destroyed"),
}

sealed interface CaptureStatus {
    data object Idle : CaptureStatus
    data object AwaitingPermission : CaptureStatus
    data object AwaitingProjectionConsent : CaptureStatus
    data class Capturing(val format: CaptureFormat) : CaptureStatus
    data class NoCapturableAudio(val format: CaptureFormat) : CaptureStatus
    data class Stopped(val reason: CaptureStopReason) : CaptureStatus
    data class Failed(val message: String) : CaptureStatus
}

data class CaptureUiState(
    val status: CaptureStatus = CaptureStatus.Idle,
)

data class CaptureEvidenceSnapshot(
    val capturedFrames: Long,
    val silentReads: Long,
    val readErrors: Long,
)
