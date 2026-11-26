package com.sonimesh.opusspike

internal const val DEFAULT_SENDER_GAIN = 100
internal const val MAX_SENDER_GAIN = 200

internal fun scaleSenderPcm(input: ShortArray, output: ShortArray, gain: Int): ShortArray {
    require(input.size == output.size)
    require(gain in 0..MAX_SENDER_GAIN)
    if (gain == DEFAULT_SENDER_GAIN) return input
    for (index in input.indices) {
        output[index] = (input[index].toInt() * gain / 100)
            .coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()).toShort()
    }
    return output
}
