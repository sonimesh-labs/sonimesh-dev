package com.sonimesh.opusspike

import com.sonimesh.sender.protocol.PairingQr

/** Public, loopback-only fixture shared with paired-audio-lab. */
internal fun labInvitation(pinText: String): ByteArray {
    val text = pinText.trim()
    require(text.length in PairingQr.SPKI_MIN_BYTES * 2..PairingQr.SPKI_MAX_BYTES * 2 &&
        text.length % 2 == 0 && text.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) {
        "invalid desktop pin"
    }
    val pin = ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
    val output = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
    val size = PairingQr.encode(1, ByteArray(16) { 0x71 }, ByteArray(32) { 0x42 },
        "127.0.0.1", 47610, pin, 120_100, output)
    return output.copyOf(size)
}
