package com.sonimesh.sender.capture

/** Detects a capture source that repeatedly returns no frames. */
internal class CaptureReadProgress(private val timeoutMillis: Long) {
    private var emptySince: Long? = null

    init {
        require(timeoutMillis > 0)
    }

    fun stalled(empty: Boolean, nowMillis: Long): Boolean {
        require(nowMillis >= 0)
        if (!empty) {
            emptySince = null
            return false
        }
        val start = emptySince
        if (start == null || nowMillis < start) {
            emptySince = nowMillis
            return false
        }
        return nowMillis - start >= timeoutMillis
    }
}

/** Runs one AudioRecord read and records non-teardown exceptions as evidence. */
internal inline fun readCaptureSamples(
    evidence: CaptureEvidence,
    shouldRecordFailure: () -> Boolean,
    read: () -> Int,
): Int {
    return try {
        read()
    } catch (error: RuntimeException) {
        if (shouldRecordFailure()) {
            evidence.recordReadError()
        }
        throw error
    }
}

/** Returns completed frames only for one bounded, channel-aligned positive read. */
internal fun completedCaptureFrames(
    samplesRead: Int,
    requestedSamples: Int,
    channels: Int,
): Int? {
    if (requestedSamples <= 0 || channels <= 0 || requestedSamples % channels != 0) {
        return null
    }
    if (samplesRead !in 1..requestedSamples || samplesRead % channels != 0) {
        return null
    }
    return samplesRead / channels
}
