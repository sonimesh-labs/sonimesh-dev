package com.sonimesh.opusspike

import java.net.ConnectException

internal fun <T> connectPairedWithRetry(
    running: () -> Boolean,
    wait: (Long) -> Unit,
    onRetry: () -> Unit = {},
    connect: () -> T,
): T {
    repeat(6) { attempt ->
        check(running()) { "capture stopped before pairing" }
        try {
            return connect()
        } catch (error: ConnectException) {
            if (attempt == 5 || !running()) throw error
            onRetry()
            repeat(20) {
                check(running()) { "capture stopped before pairing" }
                wait(50)
            }
        }
    }
    error("unreachable")
}
