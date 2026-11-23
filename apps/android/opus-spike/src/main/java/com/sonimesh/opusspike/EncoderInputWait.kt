package com.sonimesh.opusspike

/** Tracks how long one complete PCM frame has waited for an encoder input buffer. */
internal class EncoderInputWait {
    private var startedAt: Long? = null

    fun shouldIdle(nowMillis: Long, output: EncoderTailOutput): Boolean {
        val start = startedAt ?: nowMillis.also { startedAt = it }
        if (nowMillis < start || nowMillis - start >= TIMEOUT_MS) throw EncoderStalledException()
        return output == EncoderTailOutput.Empty
    }

    fun reset() { startedAt = null }

    private companion object { const val TIMEOUT_MS = 2_000L }
}

internal class EncoderStalledException : IllegalStateException("encoder input stalled")
