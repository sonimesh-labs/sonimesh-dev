package com.sonimesh.opusspike

internal class LabOpusDurationException : IllegalStateException()

/** Checks packet duration only; the receiver remains responsible for Opus decoding. */
internal fun requireLabOpusDuration(packet: ByteArray) {
    if (packet.isEmpty()) throw LabOpusDurationException()
    val toc = packet[0].toInt() and 0xff
    val config = toc ushr 3
    val frameUs = when {
        config >= 16 -> 2_500 shl (config and 3)
        config >= 12 -> 10_000 shl (config and 1)
        config and 3 == 3 -> 60_000
        else -> 10_000 shl (config and 3)
    }
    val frames = when (toc and 3) {
        0 -> 1
        1, 2 -> 2
        else -> if (packet.size > 1) packet[1].toInt() and 0x3f else 0
    }
    if (frames == 0 || frameUs * frames != 20_000) throw LabOpusDurationException()
}
