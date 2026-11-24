package com.sonimesh.sender.capture

/** Maximum absolute PCM16 sample in a completed read. */
internal fun capturePeakMagnitude(samples: ShortArray, count: Int): Int {
    require(count in 1..samples.size)
    var peak = 0
    for (index in 0 until count) {
        val sample = samples[index].toInt()
        peak = maxOf(peak, kotlin.math.abs(sample))
    }
    return peak
}
