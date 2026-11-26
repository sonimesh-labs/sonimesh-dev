package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity

internal fun attemptPairedStop(
    identity: StreamIdentity?,
    controlFailed: Boolean,
    stop: (StreamIdentity) -> StreamControlResult,
): Result<StreamControlResult>? {
    if (identity == null || controlFailed) return null
    return try {
        Result.success(stop(identity))
    } catch (error: Exception) {
        Result.failure(error)
    }
}

internal fun captureStopWarning(
    hasTerminalStatus: Boolean,
    result: Result<StreamControlResult>?,
    tailFailed: Boolean = false,
): String? {
    if (hasTerminalStatus) return null
    val status = result?.getOrNull()
    val stopUnconfirmed = result != null && (status !is StreamControlResult.Status || status.state != 4)
    return when {
        tailFailed && stopUnconfirmed -> "Capture stopped locally, but the end of the audio may be missing and the desktop did not confirm the stop. Check or restart the listener."
        stopUnconfirmed -> "Capture stopped locally, but the desktop did not confirm the stop. Check or restart the listener."
        tailFailed -> "Capture stopped, but the end of the audio may be missing."
        else -> null
    }
}
