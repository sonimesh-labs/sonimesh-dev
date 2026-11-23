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
): String? {
    if (hasTerminalStatus || result == null) return null
    val status = result.getOrNull()
    if (status is StreamControlResult.Status && status.state == 4) return null
    return "Capture stopped locally, but the desktop did not confirm the stop. Check or restart the listener."
}
