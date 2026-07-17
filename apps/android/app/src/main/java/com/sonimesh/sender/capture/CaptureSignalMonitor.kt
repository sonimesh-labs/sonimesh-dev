package com.sonimesh.sender.capture

/**
 * Low-rate signal transitions derived from completed AudioRecord reads.
 *
 * This monitor deliberately retains only the current silent-read run. It does
 * not store samples or identify a source application's capture policy.
 */
class CaptureSignalMonitor(
    private val silentReadLimit: Int,
) {
    private var consecutiveSilentReads = 0
    private var noSignalReported = false

    init {
        require(silentReadLimit > 0) { "silent read limit must be positive" }
    }

    /** Records one completed, non-empty read and returns a transition if needed. */
    fun observe(silent: Boolean): CaptureSignalTransition? {
        if (silent) {
            consecutiveSilentReads++
            if (consecutiveSilentReads == silentReadLimit && !noSignalReported) {
                noSignalReported = true
                return CaptureSignalTransition.NoCapturableAudio
            }
            return null
        }

        consecutiveSilentReads = 0
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
