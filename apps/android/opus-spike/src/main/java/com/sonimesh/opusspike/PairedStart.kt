package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult

internal fun startPairedWithRetry(
    running: () -> Boolean,
    wait: (Long) -> Unit,
    start: () -> StreamControlResult,
): StreamControlResult {
    repeat(3) { attempt ->
        check(running()) { "capture stopped while starting stream" }
        val result = start()
        if (result !is StreamControlResult.Rejected || !result.retryable || attempt == 2) return result
        repeat(10) {
            check(running()) { "capture stopped while starting stream" }
            wait(50)
        }
    }
    error("unreachable")
}
