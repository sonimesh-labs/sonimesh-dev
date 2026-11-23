package com.sonimesh.opusspike

/** Tracks consecutive non-blocking reads that produce no PCM. */
internal class CaptureReadWait {
    private var startedAt: Long? = null

    fun observe(read: Int, nowMillis: Long) {
        if (read > 0) {
            startedAt = null
            return
        }
        val start = startedAt ?: nowMillis.also { startedAt = it }
        if (nowMillis < start) startedAt = nowMillis
        else if (nowMillis - start >= TIMEOUT_MS) throw CaptureReadStalledException()
    }

    private companion object { const val TIMEOUT_MS = 5_000L }
}

internal class CaptureReadStalledException : IllegalStateException("audio capture reads stalled")
