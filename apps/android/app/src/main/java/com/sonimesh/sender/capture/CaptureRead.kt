package com.sonimesh.sender.capture

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
