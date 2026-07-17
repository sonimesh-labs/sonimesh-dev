package com.sonimesh.sender.capture

const val CAPTURE_READ_DURATION_MS = 20
const val MIN_CAPTURE_SAMPLE_RATE_HZ = 8_000
const val MAX_CAPTURE_SAMPLE_RATE_HZ = 192_000
const val MAX_CAPTURE_CHANNELS = 8
const val MAX_CAPTURE_BUFFER_BYTES = 64 * 1024

data class CaptureFormat(
    val sampleRateHz: Int,
    val channels: Int,
    val bufferSizeBytes: Int,
    val minimumBufferBytes: Int,
    val readSamples: Int,
) {
    companion object {
        fun fromObserved(
            sampleRateHz: Int,
            channels: Int,
            bufferSizeBytes: Int,
            minimumBufferBytes: Int,
        ): CaptureFormat? {
            if (sampleRateHz !in MIN_CAPTURE_SAMPLE_RATE_HZ..MAX_CAPTURE_SAMPLE_RATE_HZ
                || channels !in 1..MAX_CAPTURE_CHANNELS
                || bufferSizeBytes !in 1..MAX_CAPTURE_BUFFER_BYTES
                || minimumBufferBytes !in 1..MAX_CAPTURE_BUFFER_BYTES
                || minimumBufferBytes > bufferSizeBytes
            ) {
                return null
            }

            val frames = (
                sampleRateHz.toLong() * CAPTURE_READ_DURATION_MS + 999
                ) / 1_000
            val samples = frames * channels
            if (samples > MAX_READ_SAMPLES) {
                return null
            }

            return CaptureFormat(
                sampleRateHz = sampleRateHz,
                channels = channels,
                bufferSizeBytes = bufferSizeBytes,
                minimumBufferBytes = minimumBufferBytes,
                readSamples = samples.toInt(),
            )
        }
    }
}

const val MAX_READ_SAMPLES =
    MAX_CAPTURE_SAMPLE_RATE_HZ / 1_000 * CAPTURE_READ_DURATION_MS * MAX_CAPTURE_CHANNELS

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
    val evidence: CaptureEvidenceSnapshot = CaptureEvidenceSnapshot(),
)

data class CaptureEvidenceSnapshot(
    val captureDurationMillis: Long = 0,
    /** Latest successful AudioRecord read completion relative to capture start. */
    val lastReadCompletionOffsetMillis: Long? = null,
    val capturedFrames: Long = 0,
    val silentReads: Long = 0,
    val readErrors: Long = 0,
)
