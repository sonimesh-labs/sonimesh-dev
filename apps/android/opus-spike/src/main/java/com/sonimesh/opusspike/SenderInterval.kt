package com.sonimesh.opusspike

internal data class SenderInterval(val packets: Long, val millis: Long)

/** Measures local sender progress over completed intervals, including idle periods. */
internal class SenderIntervalMeter(startMillis: Long) {
    private var previousMillis = startMillis
    private var previousPackets = 0L

    fun sample(nowMillis: Long, packets: Long): SenderInterval? {
        if (nowMillis < previousMillis || packets < previousPackets) {
            previousMillis = nowMillis
            previousPackets = packets
            return null
        }
        val elapsed = nowMillis - previousMillis
        if (elapsed < 1_000) return null
        val result = SenderInterval(packets - previousPackets, elapsed)
        previousMillis = nowMillis
        previousPackets = packets
        return result
    }
}
