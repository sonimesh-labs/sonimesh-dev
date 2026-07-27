package com.sonimesh.sender.capture

/**
 * Low-rate signal transitions derived from completed AudioRecord frames.
 *
 * This monitor deliberately retains only the current silent-frame run. It does
 * not store samples or identify a source application's capture policy.
 */
class CaptureSignalMonitor(
    private val silentFrameThreshold: Long,
) {
    private var consecutiveSilentFrames = 0L
    private var noSignalReported = false

    init {
        require(silentFrameThreshold > 0) { "silent frame threshold must be positive" }
    }

    /** Records one completed, non-empty read and returns a transition if needed. */
    fun observe(silent: Boolean, frames: Int): CaptureSignalTransition? {
        require(frames > 0) { "completed read must contain at least one frame" }

        if (silent) {
            val remainingFrames = silentFrameThreshold - consecutiveSilentFrames
            consecutiveSilentFrames += minOf(frames.toLong(), remainingFrames)
            if (consecutiveSilentFrames == silentFrameThreshold && !noSignalReported) {
                noSignalReported = true
                return CaptureSignalTransition.NoCapturableAudio
            }
            return null
        }

        consecutiveSilentFrames = 0
        if (!noSignalReported) {
            return null
        }
        noSignalReported = false
        return CaptureSignalTransition.SignalDetected
    }
}

/** Low-rate capture signal state transition suitable for UI publication. */
enum class CaptureSignalTransition {
    NoCapturableAudio,
    SignalDetected,
}
