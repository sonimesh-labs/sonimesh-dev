package com.sonimesh.opusspike

internal const val LATENCY_PROBE_RATE = 48_000
internal const val LATENCY_PROBE_FRAMES = LATENCY_PROBE_RATE * 3
internal const val LATENCY_PROBE_PERIOD = (1 shl 17) - 1

/** Seed 7, polynomial x^17 + x^3 + 1, followed by silence. */
internal fun latencyProbeSamples(): ShortArray {
    var state = 7
    return ShortArray(LATENCY_PROBE_FRAMES) { index ->
        if (index >= LATENCY_PROBE_PERIOD) {
            0
        } else {
            val head = (state shr 16) and 1
            state = ((state shl 1) or (head xor ((state shr 2) and 1))) and LATENCY_PROBE_PERIOD
            if (head == 1) 3_000 else -3_000
        }
    }
}
