package com.sonimesh.opusspike

import kotlin.math.PI
import kotlin.math.sin

internal fun testToneSamples(sampleRate: Int, channels: Int): ShortArray =
    ShortArray(sampleRate * channels) { index ->
        val frame = index / channels
        (sin(2.0 * PI * 1_000 * frame / sampleRate) * 3_000).toInt().toShort()
    }
