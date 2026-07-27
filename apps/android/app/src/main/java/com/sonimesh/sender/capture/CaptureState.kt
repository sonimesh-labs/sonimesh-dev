package com.sonimesh.sender.capture

const val CAPTURE_READ_DURATION_MS = 20
const val MIN_CAPTURE_SAMPLE_RATE_HZ = 8_000
const val MAX_CAPTURE_SAMPLE_RATE_HZ = 192_000
const val MAX_CAPTURE_CHANNELS = 8
const val MAX_CAPTURE_BUFFER_BYTES = 64 * 1024

data class CaptureFormat(
    val sampleRateHz: Int,
    val channels: Int,
    val audioSource: Int,
    val channelMask: Int,
    val bufferSizeBytes: Int,
    val minimumBufferBytes: Int,
    val readSamples: Int,
) {
    companion object {
        fun fromObserved(
            sampleRateHz: Int,
            channels: Int,
            audioSource: Int,
            channelMask: Int,
            bufferSizeBytes: Int,
            minimumBufferBytes: Int,
        ): CaptureFormat? {
            if (sampleRateHz !in MIN_CAPTURE_SAMPLE_RATE_HZ..MAX_CAPTURE_SAMPLE_RATE_HZ
                || channels !in 1..MAX_CAPTURE_CHANNELS
                || audioSource < 0
                || channelMask < 0
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
                audioSource = audioSource,
                channelMask = channelMask,
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
    data object AwaitingNotificationPermission : CaptureStatus
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
    /** Cumulative completed silent frames; this does not identify why they were silent. */
    val silentFrames: Long = 0,
    val silentReads: Long = 0,
    val readErrors: Long = 0,
)


/** True while the foreground service owns an active capture lifecycle. */
internal fun CaptureStatus.isActive(): Boolean = when (this) {
    is CaptureStatus.Capturing, is CaptureStatus.NoCapturableAudio -> true
    else -> false
}

/** True while the activity has already opened a system authorization path. */
internal fun CaptureStatus.isAwaitingAuthorization(): Boolean = when (this) {
    CaptureStatus.AwaitingPermission,
    CaptureStatus.AwaitingNotificationPermission,
    CaptureStatus.AwaitingProjectionConsent,
    -> true
    else -> false
}

/** True when a fresh capture request may begin from the current UI state. */
internal fun CaptureStatus.canStart(): Boolean = !isActive() && !isAwaitingAuthorization()
