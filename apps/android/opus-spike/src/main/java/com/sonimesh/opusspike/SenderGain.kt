package com.sonimesh.opusspike

internal const val DEFAULT_SENDER_GAIN = 100
internal const val MAX_SENDER_GAIN = 200
internal data class ClippingInterval(val clipped: Long, val accepted: Long)

internal fun scaleSenderPcm(input: ShortArray, output: ShortArray, startGain: Int, endGain: Int, clipped: BooleanArray? = null): ShortArray {
    require(input.size == output.size)
    require(input.size >= 4 && input.size % 2 == 0)
    require(startGain in 0..MAX_SENDER_GAIN && endGain in 0..MAX_SENDER_GAIN)
    if (clipped != null) clipped[0] = false
    if (startGain == DEFAULT_SENDER_GAIN && endGain == DEFAULT_SENDER_GAIN) return input
    val lastFrame = input.size / 2 - 1
    for (index in input.indices) {
        val frame = index / 2
        val gainNumerator = startGain * lastFrame + (endGain - startGain) * frame
        val scaled = input[index].toLong() * gainNumerator / (100 * lastFrame)
        if (clipped != null && (scaled < Short.MIN_VALUE || scaled > Short.MAX_VALUE)) clipped[0] = true
        output[index] = scaled.coerceIn(Short.MIN_VALUE.toLong(), Short.MAX_VALUE.toLong()).toShort()
    }
    return output
}

/** Keeps gain transitions aligned with frames actually accepted by the encoder. */
internal class SenderGainProcessor(frameSamples: Int) {
    private val output = ShortArray(frameSamples)
    private val silence = ShortArray(frameSamples)
    private var acceptedGain = DEFAULT_SENDER_GAIN
    private var preparedGain = DEFAULT_SENDER_GAIN
    private val preparedClipped = booleanArrayOf(false)
    var clippedFrames = 0L
        private set
    private var intervalClippedFrames = 0L
    private var intervalAcceptedFrames = 0L

    fun prepare(input: ShortArray, muted: Boolean, targetGain: Int): ShortArray {
        require(input.size == output.size)
        require(targetGain in 0..MAX_SENDER_GAIN)
        preparedGain = if (muted) 0 else targetGain
        if (muted) {
            preparedClipped[0] = false
            return silence
        }
        return scaleSenderPcm(input, output, acceptedGain, preparedGain, preparedClipped)
    }

    fun accepted() {
        acceptedGain = preparedGain
        if (intervalAcceptedFrames < Long.MAX_VALUE) intervalAcceptedFrames++
        if (preparedClipped[0]) {
            if (clippedFrames < Long.MAX_VALUE) clippedFrames++
            if (intervalClippedFrames < Long.MAX_VALUE) intervalClippedFrames++
        }
    }

    fun takeClippedInterval(): ClippingInterval {
        val count = ClippingInterval(intervalClippedFrames, intervalAcceptedFrames)
        intervalClippedFrames = 0
        intervalAcceptedFrames = 0
        return count
    }
}
