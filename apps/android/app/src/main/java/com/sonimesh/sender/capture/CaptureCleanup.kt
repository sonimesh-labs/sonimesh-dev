package com.sonimesh.sender.capture

/** Result of attempting every bounded cleanup step. */
internal data class CaptureCleanupResult(
    val ordinaryFailureCount: Int,
    val fatalFailure: Throwable?,
)

/** Attempts every bounded step while retaining the first fatal failure. */
internal fun collectCaptureCleanupFailures(vararg steps: () -> Unit): CaptureCleanupResult {
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

    return CaptureCleanupResult(ordinaryFailureCount, fatalFailure)
}

/** Keeps the first fatal failure and attaches a later cleanup failure to it. */
internal fun mergeCaptureFatalFailures(
    primary: Throwable?,
    secondary: Throwable?,
): Throwable? {
    if (primary == null) {
        return secondary
    }
    if (secondary != null && secondary !== primary) {
        primary.addSuppressed(secondary)
    }
    return primary
}

/** Thread-safe holder for fatal failures deferred until bounded teardown completes. */
internal class DeferredCaptureFatalFailure {
    private var failure: Throwable? = null

    @Synchronized
    fun retain(next: Throwable?) {
        failure = mergeCaptureFatalFailures(failure, next)
    }

    @Synchronized
    fun take(): Throwable? {
        val retained = failure
        failure = null
        return retained
    }
}

/** Runs every bounded cleanup step and rethrows the first deferred fatal failure. */
internal fun runAllCaptureCleanupSteps(vararg steps: () -> Unit): Int {
    val result = collectCaptureCleanupFailures(*steps)
    result.fatalFailure?.let { throw it }
    return result.ordinaryFailureCount
}
