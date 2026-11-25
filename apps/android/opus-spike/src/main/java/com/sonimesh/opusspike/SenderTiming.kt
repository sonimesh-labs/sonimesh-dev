package com.sonimesh.opusspike

internal data class SenderTimingSnapshot(val intervals: Long, val underTenMillis: Long)

/** Measures completed send calls using one monotonic clock across sender threads. */
internal class SenderTiming {
    private var previousNanos: Long? = null
    private var intervals = 0L
    private var underTenMillis = 0L

    @Synchronized
    fun record(nowNanos: Long): SenderTimingSnapshot {
        val previous = previousNanos
        if (previous != null && nowNanos >= previous) {
            intervals = incrementSaturated(intervals)
            if (nowNanos - previous < 10_000_000L) underTenMillis = incrementSaturated(underTenMillis)
        }
        previousNanos = nowNanos
        return SenderTimingSnapshot(intervals, underTenMillis)
    }

    @Synchronized
    fun snapshot(): SenderTimingSnapshot = SenderTimingSnapshot(intervals, underTenMillis)
}
