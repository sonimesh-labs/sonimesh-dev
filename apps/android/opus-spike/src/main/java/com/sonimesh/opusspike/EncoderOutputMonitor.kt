package com.sonimesh.opusspike

/** Observes accepted signal and actual packets, without retaining audio samples. */
internal class EncoderOutputMonitor {
    private var startedAt: Long? = null
    private var warning = false

    fun accepted(nowMillis: Long, hasSignal: Boolean): Boolean? {
        if (!hasSignal) return reset()
        val start = startedAt ?: nowMillis.also { startedAt = it }
        if (nowMillis < start) {
            startedAt = nowMillis
            return setWarning(false)
        }
        return if (nowMillis - start >= 2_000) setWarning(true) else null
    }

    fun packet(): Boolean? = reset()

    private fun reset(): Boolean? {
        startedAt = null
        return setWarning(false)
    }

    private fun setWarning(value: Boolean): Boolean? {
        if (warning == value) return null
        warning = value
        return value
    }
}
