package com.sonimesh.sender.capture

/** Runs every bounded cleanup step and rethrows the first deferred fatal failure. */
internal fun runAllCaptureCleanupSteps(vararg steps: () -> Unit): Int {
    var ordinaryFailureCount = 0
    var fatalFailure: Throwable? = null

    for (step in steps) {
        try {
            step()
        } catch (_: Exception) {
            ordinaryFailureCount += 1
            // Ordinary platform cleanup failures must not strand later resources.
        } catch (failure: Throwable) {
            if (fatalFailure == null) {
                fatalFailure = failure
            }
        }
    }

    fatalFailure?.let { throw it }
    return ordinaryFailureCount
}
