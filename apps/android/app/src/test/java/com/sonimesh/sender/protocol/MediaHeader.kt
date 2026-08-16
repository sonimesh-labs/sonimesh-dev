package com.sonimesh.sender.protocol

import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Fixed portion of the version-one media header. */
const val MEDIA_HEADER_LEN = 36

/** Maximum declared header length, including any extended header bytes. */
const val MAX_HEADER_LEN = 128

/** Maximum complete media datagram to avoid relying on IP fragmentation. */
const val MAX_MEDIA_DATAGRAM_LEN = 1_200

/** Maximum encrypted payload with a fixed header, including the authentication tag. */
const val MAX_ENCRYPTED_PAYLOAD_LEN = MAX_MEDIA_DATAGRAM_LEN - MEDIA_HEADER_LEN

/** Packet-family marker (`SNMS` in network byte order). */
const val MEDIA_MAGIC = 0x534E4D53

/** Current data-plane protocol version. */
const val MEDIA_VERSION: Byte = 1

/**
 * Host-JVM mirror of `sonimesh-protocol`'s version-one `MediaHeader` and its
 * `MediaHeader::encode` bounds, shared by the wire-format vector test so the
 * Android sender and the Rust data plane encode the same 36-byte header used as
 * AEAD associated data.
 */
data class MediaHeader(
    val flags: Byte,
    val sessionId: Long,
    val streamId: Int,
    val sequence: Int,
    val sampleTimestamp: Long,
    val keyEpoch: Short,
) {
    /**
     * Encodes the fixed version-one header for an encrypted payload.
     *
     * @throws IllegalArgumentException when the encrypted payload exceeds the
     *   hard datagram limit, mirroring Rust `PacketParseError::PayloadTooLarge`.
     */
    fun encode(payloadBytes: Int): ByteArray {
        require(payloadBytes in 0..MAX_ENCRYPTED_PAYLOAD_LEN) {
            "media payload exceeds the hard limit"
        }
        return ByteBuffer.allocate(MEDIA_HEADER_LEN)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(MEDIA_MAGIC)
            .put(MEDIA_VERSION)
            .put(flags)
            .putShort(MEDIA_HEADER_LEN.toShort())
            .putLong(sessionId)
            .putInt(streamId)
            .putInt(sequence)
            .putLong(sampleTimestamp)
            .putShort(payloadBytes.toShort())
            .putShort(keyEpoch)
            .array()
    }
}
