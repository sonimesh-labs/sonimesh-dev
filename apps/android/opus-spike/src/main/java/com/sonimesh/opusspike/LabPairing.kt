package com.sonimesh.opusspike

import com.sonimesh.sender.protocol.PairingQr

/** Public test fixture shared with paired-audio-lab. */
internal fun labInvitation(pinText: String, controlHost: String = "127.0.0.1", controlPort: Int = 47610): ByteArray {
    val text = pinText.trim()
    require(text.length in PairingQr.SPKI_MIN_BYTES * 2..PairingQr.SPKI_MAX_BYTES * 2 &&
        text.length % 2 == 0 && text.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) {
        "invalid desktop pin"
    }
    val pin = ByteArray(text.length / 2) { text.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
    val output = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
    val size = PairingQr.encode(1, ByteArray(16) { 0x71 }, ByteArray(32) { 0x42 },
        controlHost, controlPort, pin, 120_100, output)
    return output.copyOf(size)
}

internal fun labHexToBytes(text: String): ByteArray {
    val normalized = text.trim()
    require(normalized.isNotEmpty() && normalized.length % 2 == 0)
    require(normalized.length <= PairingQr.MAX_PAYLOAD_BYTES * 2)
    return ByteArray(normalized.length / 2) { index ->
        val high = normalized[index * 2].digitToIntOrNull(16)
        val low = normalized[index * 2 + 1].digitToIntOrNull(16)
        require(high != null && low != null)
        ((high shl 4) or low).toByte()
    }
}

internal const val LAB_INVITATION_PREFIX = "SONIMESH-LAB1:"
internal const val MAX_LAB_INVITATION_TEXT = PairingQr.MAX_PAYLOAD_BYTES * 2 + LAB_INVITATION_PREFIX.length + 6

internal data class LabInvitationText(val payload: ByteArray, val mediaPort: Int?)

internal fun parseLabInvitationText(text: String): LabInvitationText {
    val normalized = text.trim()
    require(normalized.length <= MAX_LAB_INVITATION_TEXT)
    if (!normalized.startsWith(LAB_INVITATION_PREFIX)) {
        return LabInvitationText(labHexToBytes(normalized), null)
    }
    val separator = normalized.indexOf(':', LAB_INVITATION_PREFIX.length)
    require(separator >= 0)
    val port = requireNotNull(parseLabMediaPort(normalized.substring(LAB_INVITATION_PREFIX.length, separator)))
    return LabInvitationText(labHexToBytes(normalized.substring(separator + 1)), port)
}

internal fun labHex(bytes: ByteArray): String = buildString(bytes.size * 2) {
    bytes.forEach { append("%02x".format(it.toInt() and 0xff)) }
}
