package com.sonimesh.sender.protocol

/**
 * Android implementation of `sonimesh-protocol`'s bounded pairing QR payload codec,
 * so the Android sender can encode and
 * decodes exactly the version-one wire layout pinned in
 * `docs/protocol/pairing-qr-vectors.md`.
 *
 * The payload is scanned before any TLS connection exists, so every length is
 * bounded before any data is copied and a decoded view exists only when the
 * declared lengths exactly match the total size. Decoding copies the ID,
 * secret and SPKI into mutable arrays owned by the caller, who also owns the
 * input buffer. This codec performs no TLS, pairing-record consumption, proof
 * exchange, or trust storage.
 *
 * @throws IllegalArgumentException when a field is outside the documented
 *   bounds or an encoded length does not fit its declared width, mirroring
 *   Rust `QrPayloadError`.
 */
class PairingQr private constructor(
    val protocolVersion: Int,
    val pairingId: ByteArray,
    val pairingSecret: ByteArray,
    val controlHost: String,
    val controlPort: Int,
    val desktopTlsSpki: ByteArray,
    val expiresAtMs: Long,
) {
    companion object {
        const val PAYLOAD_VERSION: Int = 1
        const val PREFIX_BYTES: Int = 67
        const val HOST_MAX_BYTES: Int = 255
        const val SPKI_MIN_BYTES: Int = 32
        const val SPKI_MAX_BYTES: Int = 512
        const val MAX_PAYLOAD_BYTES: Int = PREFIX_BYTES + HOST_MAX_BYTES + SPKI_MAX_BYTES
        private val MAGIC = "SMQR".encodeToByteArray()

        /**
         * Validates and encodes one invitation into [out], returning the
         * written byte count.
         */
        fun encode(
            protocolVersion: Int,
            pairingId: ByteArray,
            pairingSecret: ByteArray,
            controlHost: String,
            controlPort: Int,
            desktopTlsSpki: ByteArray,
            expiresAtMs: Long,
            out: ByteArray,
        ): Int {
            require(protocolVersion == CURRENT_PROTOCOL_VERSION) {
                "unsupported pairing protocol version $protocolVersion"
            }
            require(pairingId.size == PAIRING_ID_BYTES) { "pairing id length is invalid" }
            require(pairingSecret.size == PAIRING_SECRET_BYTES) {
                "pairing secret length is invalid"
            }
            require(controlHost.isNotEmpty() && controlHost.length <= HOST_MAX_BYTES) {
                "pairing QR host length is invalid"
            }
            require(controlPort in 1..UShort.MAX_VALUE.toInt()) {
                "pairing QR control port is zero"
            }
            require(desktopTlsSpki.size in SPKI_MIN_BYTES..SPKI_MAX_BYTES) {
                "pairing QR desktop SPKI length is invalid"
            }
            require(expiresAtMs != 0L) { "pairing QR expiry is zero" }

            val hostBytes = controlHost.encodeToByteArray()
            require(hostBytes.size == controlHost.length) {
                "pairing QR host is not UTF-8 text without control characters"
            }
            hostBytes.forEach { byte ->
                require(byte.toInt() in 0x20..0x7E) {
                    "pairing QR host is not UTF-8 text without control characters"
                }
            }

            val encodedLength = PREFIX_BYTES + hostBytes.size + desktopTlsSpki.size
            require(out.size >= encodedLength) {
                "pairing QR payload exceeds the hard limit"
            }

            out[0] = MAGIC[0]
            out[1] = MAGIC[1]
            out[2] = MAGIC[2]
            out[3] = MAGIC[3]
            out[4] = PAYLOAD_VERSION.toByte()
            out[5] = protocolVersion.toByte()
            out[6] = hostBytes.size.toByte()
            out[7] = (controlPort shr 8).toByte()
            out[8] = controlPort.toByte()
            out[9] = (desktopTlsSpki.size shr 8).toByte()
            out[10] = desktopTlsSpki.size.toByte()
            pairingId.copyInto(out, PAIRING_ID_OFFSET)
            pairingSecret.copyInto(out, PAIRING_SECRET_OFFSET)
            for (shift in 0 until 8) {
                out[EXPIRY_OFFSET + shift] =
                    (expiresAtMs shr ((7 - shift) * 8)).toByte()
            }
            hostBytes.copyInto(out, PREFIX_BYTES)
            desktopTlsSpki.copyInto(out, PREFIX_BYTES + hostBytes.size)
            return encodedLength
        }

        /** Parses an untrusted payload into validated copied fields. */
        fun decode(payload: ByteArray): PairingQr {
            require(payload.size <= MAX_PAYLOAD_BYTES) {
                "pairing QR payload exceeds the hard limit"
            }
            require(payload.size >= PREFIX_BYTES) { "pairing QR payload is truncated" }
            require(payload[0] == MAGIC[0] && payload[1] == MAGIC[1] &&
                payload[2] == MAGIC[2] && payload[3] == MAGIC[3]) {
                "pairing QR payload has an invalid magic value"
            }
            require(payload[4].toInt() == PAYLOAD_VERSION) {
                "unsupported pairing QR payload version ${payload[4]}"
            }
            val protocolVersion = payload[5].toInt() and 0xFF
            require(protocolVersion == CURRENT_PROTOCOL_VERSION) {
                "unsupported pairing protocol version $protocolVersion"
            }
            val hostLen = payload[6].toInt() and 0xFF
            require(hostLen != 0) { "pairing QR host length is invalid" }
            val controlPort = ((payload[7].toInt() and 0xFF) shl 8) or
                (payload[8].toInt() and 0xFF)
            require(controlPort != 0) { "pairing QR control port is zero" }
            val spkiLen = ((payload[9].toInt() and 0xFF) shl 8) or
                (payload[10].toInt() and 0xFF)
            require(spkiLen in SPKI_MIN_BYTES..SPKI_MAX_BYTES) {
                "pairing QR desktop SPKI length is invalid"
            }
            var expiresAtMs = 0L
            for (shift in 0 until 8) {
                expiresAtMs = (expiresAtMs shl 8) or (payload[EXPIRY_OFFSET + shift].toLong() and 0xFF)
            }
            require(expiresAtMs != 0L) { "pairing QR expiry is zero" }
            require(PREFIX_BYTES + hostLen + spkiLen == payload.size) {
                "pairing QR payload length does not match its lengths"
            }

            val hostEnd = PREFIX_BYTES + hostLen
            val hostBytes = payload.copyOfRange(PREFIX_BYTES, hostEnd)
            hostBytes.forEach { byte ->
                require(byte.toInt() in 0x20..0x7E) {
                    "pairing QR host is not UTF-8 text without control characters"
                }
            }
            val controlHost = hostBytes.decodeToString()
            require(controlHost.length == hostLen) {
                "pairing QR host is not UTF-8 text without control characters"
            }

            val pairingId = payload.copyOfRange(PAIRING_ID_OFFSET, PAIRING_ID_OFFSET + PAIRING_ID_BYTES)
            val pairingSecret = payload.copyOfRange(
                PAIRING_SECRET_OFFSET,
                PAIRING_SECRET_OFFSET + PAIRING_SECRET_BYTES,
            )
            val spkiStart = hostEnd
            val spki = payload.copyOfRange(spkiStart, spkiStart + spkiLen)
            return PairingQr(
                protocolVersion = protocolVersion,
                pairingId = pairingId,
                pairingSecret = pairingSecret,
                controlHost = controlHost,
                controlPort = controlPort,
                desktopTlsSpki = spki,
                expiresAtMs = expiresAtMs,
            )
        }

        private const val CURRENT_PROTOCOL_VERSION = 1
        private const val PAIRING_ID_BYTES = 16
        private const val PAIRING_SECRET_BYTES = 32
        private const val PAIRING_ID_OFFSET = 11
        private const val PAIRING_SECRET_OFFSET = 27
        private const val EXPIRY_OFFSET = 59
    }
}
