package com.sonimesh.opusspike

internal const val DEFAULT_SENDER_GAIN = 100
internal const val MAX_SENDER_GAIN = 200

internal fun scaleSenderPcm(input: ShortArray, output: ShortArray, startGain: Int, endGain: Int): ShortArray {
    require(input.size == output.size)
    require(input.size >= 4 && input.size % 2 == 0)
    require(startGain in 0..MAX_SENDER_GAIN && endGain in 0..MAX_SENDER_GAIN)
    if (startGain == DEFAULT_SENDER_GAIN && endGain == DEFAULT_SENDER_GAIN) return input
    val lastFrame = input.size / 2 - 1
    for (index in input.indices) {
        val frame = index / 2
        val gainNumerator = startGain * lastFrame + (endGain - startGain) * frame
        output[index] = (input[index].toLong() * gainNumerator / (100 * lastFrame))
            .coerceIn(Short.MIN_VALUE.toLong(), Short.MAX_VALUE.toLong()).toShort()
    }
    return output
}

/** Keeps gain transitions aligned with frames actually accepted by the encoder. */
internal class SenderGainProcessor(frameSamples: Int) {
    private val output = ShortArray(frameSamples)
    private val silence = ShortArray(frameSamples)
    private var acceptedGain = DEFAULT_SENDER_GAIN
    private var preparedGain = DEFAULT_SENDER_GAIN

    fun prepare(input: ShortArray, muted: Boolean, targetGain: Int): ShortArray {
        require(input.size == output.size)
        require(targetGain in 0..MAX_SENDER_GAIN)
        preparedGain = if (muted) 0 else targetGain
        if (muted) return silence
        return scaleSenderPcm(input, output, acceptedGain, preparedGain)
    }

    fun accepted() {
        acceptedGain = preparedGain
    }
}
