package com.sonimesh.sender.capture

/** Limits UI evidence snapshots to a bounded, low publication cadence. */
class CaptureEvidencePublishThrottle(
    private val minimumIntervalMillis: Long,
) {
    private var lastPublishedAtMillis: Long? = null

    init {
        require(minimumIntervalMillis > 0) { "evidence publication interval must be positive" }
    }

    /** Returns whether one evidence snapshot may be published at this monotonic time. */
    fun shouldPublish(nowMillis: Long): Boolean {
        require(nowMillis >= 0) { "evidence publication time must be monotonic" }
        val previous = lastPublishedAtMillis
        if (previous != null
            && nowMillis >= previous
            && nowMillis - previous < minimumIntervalMillis
        ) {
            return false
        }
        // Rebase after a nonnegative clock regression instead of suppressing
        // every publication until the previous timestamp is reached again.
        lastPublishedAtMillis = nowMillis
        return true
    }
}
