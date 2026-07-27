package com.sonimesh.sender.capture

/** Runs each bounded final-cleanup step even when an earlier platform call fails. */
internal fun runAllCaptureCleanupSteps(vararg steps: () -> Unit) {
    for (step in steps) {
        runCatching(step)
    }
}
