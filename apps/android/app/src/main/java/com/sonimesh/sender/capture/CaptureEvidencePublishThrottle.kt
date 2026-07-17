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
        val previous = lastPublishedAtMillis
        if (previous != null && nowMillis - previous < minimumIntervalMillis) {
            return false
        }
        lastPublishedAtMillis = nowMillis
        return true
    }
}
