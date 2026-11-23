package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult

internal class PairedStartRejectedException(val retryable: Boolean) : Exception()
internal class PairedStartStatusException : Exception()

internal fun requirePairedStart(result: StreamControlResult) {
    when (result) {
        is StreamControlResult.Rejected -> throw PairedStartRejectedException(result.retryable)
        is StreamControlResult.Status -> if (result.state != 2) throw PairedStartStatusException()
    }
}

internal fun startPairedWithRetry(
    running: () -> Boolean,
    wait: (Long) -> Unit,
    onRetry: () -> Unit = {},
    start: () -> StreamControlResult,
): StreamControlResult {
    repeat(3) { attempt ->
        check(running()) { "capture stopped while starting stream" }
        val result = start()
        if (result !is StreamControlResult.Rejected || !result.retryable || attempt == 2) return result
        onRetry()
        repeat(10) {
            check(running()) { "capture stopped while starting stream" }
            wait(50)
        }
    }
    error("unreachable")
}
