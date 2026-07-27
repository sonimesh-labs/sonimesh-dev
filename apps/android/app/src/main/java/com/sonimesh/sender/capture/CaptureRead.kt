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
