package com.sonimesh.opusspike

internal enum class EncoderTailOutput { Empty, Progress, End }

/** Drains pending encoder output while trying to submit and observe end-of-stream. */
internal fun drainEncoderTail(
    nowMillis: () -> Long,
    submitEnd: () -> Boolean,
    pollOutput: () -> EncoderTailOutput,
    idle: () -> Unit,
): Boolean {
    val deadline = nowMillis() + 250
    var submitted = false
    while (nowMillis() < deadline) {
        if (!submitted) submitted = submitEnd()
        when (pollOutput()) {
            EncoderTailOutput.End -> return submitted
            EncoderTailOutput.Empty -> idle()
            EncoderTailOutput.Progress -> Unit
        }
    }
    return false
}
