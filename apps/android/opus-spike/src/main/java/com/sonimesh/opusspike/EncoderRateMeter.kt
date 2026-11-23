package com.sonimesh.opusspike

import kotlin.math.roundToLong

/** Measures cumulative encoded payload against elapsed capture time. */
internal class EncoderRateMeter(private val startedAtMillis: Long) {
    private var bytes = 0L

    fun record(packetBytes: Int) {
        require(packetBytes > 0)
        bytes += packetBytes
    }

    fun averageKbps(nowMillis: Long): Long? {
        val elapsed = nowMillis - startedAtMillis
        if (bytes == 0L || elapsed < 1_000) return null
        return (bytes.toDouble() * 8.0 / elapsed).roundToLong()
    }
}
