package com.sonimesh.sender.protocol

/**
 * Android implementation of `sonimesh-protocol`'s bounded pairing proof-exchange
 * frame codec, using the version-one wire layout pinned in
 * `docs/protocol/pairing-proof-frame-vectors.md`.
 *
 * After the pinned TLS 1.3 handshake, each peer sends one 32-byte transcript
 * proof (ADR 0008). The frame is fixed-length, so every field is validated
 * before use and no declared length can disagree with the total size.
 * Decoding copies into fresh arrays; this codec performs no TLS, derivation,
 * verification, pairing-record consumption, or trust storage.
 *
 * @throws IllegalArgumentException when a field is outside the documented
 *   layout, mirroring Rust `PairingProofFrameError`.
 */
class PairingProofFrame private constructor(
    val protocolVersion: Int,
    val role: Int,
    val pairingId: ByteArray,
    val proof: ByteArray,
) {
    companion object {
        const val FRAME_VERSION: Int = 1
        const val FRAME_BYTES: Int = 55
        const val ROLE_ANDROID_CLIENT: Int = 0
        const val ROLE_DESKTOP_SERVER: Int = 1
        private val MAGIC = "SMPE".encodeToByteArray()

        /** Validates and encodes one proof frame into [out]. */
        fun encode(
            protocolVersion: Int,
            role: Int,
            pairingId: ByteArray,
            proof: ByteArray,
            out: ByteArray,
        ): Int {
            require(protocolVersion == CURRENT_PROTOCOL_VERSION) {
                "unsupported pairing protocol version $protocolVersion"
            }
            require(role == ROLE_ANDROID_CLIENT || role == ROLE_DESKTOP_SERVER) {
                "invalid pairing role byte $role"
            }
            require(pairingId.size == PAIRING_ID_BYTES) { "pairing id length is invalid" }
            require(proof.size == PROOF_BYTES) { "proof length is invalid" }
            require(out.size >= FRAME_BYTES) {
                "pairing proof frame output buffer is too small"
            }

            out[0] = MAGIC[0]
            out[1] = MAGIC[1]
            out[2] = MAGIC[2]
            out[3] = MAGIC[3]
            out[4] = FRAME_VERSION.toByte()
            out[5] = protocolVersion.toByte()
            out[6] = role.toByte()
            pairingId.copyInto(out, PAIRING_ID_OFFSET)
            proof.copyInto(out, PROOF_OFFSET)
            return FRAME_BYTES
        }

        /** Parses an untrusted frame into validated copied fields. */
        fun decode(frame: ByteArray): PairingProofFrame {
            require(frame.size == FRAME_BYTES) {
                "pairing proof frame is truncated or oversized"
            }
            require(frame[0] == MAGIC[0] && frame[1] == MAGIC[1] &&
                frame[2] == MAGIC[2] && frame[3] == MAGIC[3]) {
                "pairing proof frame has an invalid magic value"
            }
            require(frame[4].toInt() == FRAME_VERSION) {
                "unsupported pairing proof frame version ${frame[4]}"
            }
            val protocolVersion = frame[5].toInt() and 0xFF
            require(protocolVersion == CURRENT_PROTOCOL_VERSION) {
                "unsupported pairing protocol version $protocolVersion"
            }
            val role = frame[6].toInt() and 0xFF
            require(role == ROLE_ANDROID_CLIENT || role == ROLE_DESKTOP_SERVER) {
                "invalid pairing role byte $role"
            }
            return PairingProofFrame(
                protocolVersion = protocolVersion,
                role = role,
                pairingId = frame.copyOfRange(PAIRING_ID_OFFSET, PAIRING_ID_OFFSET + PAIRING_ID_BYTES),
                proof = frame.copyOfRange(PROOF_OFFSET, PROOF_OFFSET + PROOF_BYTES),
            )
        }

        private const val CURRENT_PROTOCOL_VERSION = 1
        private const val PAIRING_ID_BYTES = 16
        private const val PROOF_BYTES = 32
        private const val PAIRING_ID_OFFSET = 7
        private const val PROOF_OFFSET = 23
    }
}
