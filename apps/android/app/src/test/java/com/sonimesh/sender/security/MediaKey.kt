package com.sonimesh.sender.security

import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Derived media key byte length (256-bit). */
const val MEDIA_KEY_BYTES = 32

/** ChaCha20-Poly1305 nonce byte length (96-bit). */
const val NONCE_BYTES = 12

/** Current data-plane protocol version. */
const val PROTOCOL_VERSION: Byte = 1

/** HKDF salt for the media root. */
val ROOT_SALT = "sonimesh media root v1".encodeToByteArray()

/** HKDF info label for one stream key. */
val STREAM_KEY_LABEL = "sonimesh media stream key v1".encodeToByteArray()

/** Direction bound into a media key and packet nonce. */
enum class MediaDirection(val byte: Byte) {
    /** Android sender to desktop receiver. */
    ANDROID_TO_DESKTOP(0),
    /** Desktop sender to Android receiver. */
    DESKTOP_TO_ANDROID(1),
}

/**
 * Media fields that select a unique per-stream AEAD key, mirroring
 * `sonimesh-protocol`'s `MediaKeyContext`.
 *
 * @throws IllegalArgumentException when the session or stream identifier is
 *   zero, mirroring Rust `MediaCryptoError::InvalidContext`.
 */
data class MediaKeyContext(
    val sessionId: Long,
    val streamId: Int,
    val keyEpoch: Short,
    val direction: MediaDirection,
) {
    init {
        require(sessionId != 0L && streamId != 0) {
            "media key context is invalid"
        }
    }
}

/**
 * Host-JVM mirror of `sonimesh-protocol`'s media key derivation and nonce
 * construction, shared by the vector test so the Android sender and the Rust
 * data plane derive the same per-stream key and 96-bit packet nonce.
 */
object MediaKey {
    /** Derives the 256-bit per-stream AEAD key (HKDF-SHA256). */
    fun deriveMediaKey(sessionSecret: ByteArray, context: MediaKeyContext): ByteArray {
        require(sessionSecret.size == MEDIA_KEY_BYTES) {
            "media session secret must be 32 bytes"
        }
        val extract = Mac.getInstance("HmacSHA256")
        extract.init(SecretKeySpec(ROOT_SALT, "HmacSHA256"))
        val pseudorandomKey = extract.doFinal(sessionSecret)

        val info = ByteBuffer.allocate(STREAM_KEY_LABEL.size + 1 + 8 + 4 + 2 + 1)
            .order(ByteOrder.BIG_ENDIAN)
            .put(STREAM_KEY_LABEL)
            .put(PROTOCOL_VERSION)
            .putLong(context.sessionId)
            .putInt(context.streamId)
            .putShort(context.keyEpoch)
            .put(context.direction.byte)
            .array()
        val expand = Mac.getInstance("HmacSHA256")
        expand.init(SecretKeySpec(pseudorandomKey, "HmacSHA256"))
        return expand.doFinal(info + byteArrayOf(1)).copyOf(MEDIA_KEY_BYTES)
    }

    /** Builds the exact 96-bit ChaCha20-Poly1305 nonce for one packet sequence. */
    fun mediaNonce(context: MediaKeyContext, sequence: Int): ByteArray =
        ByteBuffer.allocate(NONCE_BYTES)
            .order(ByteOrder.BIG_ENDIAN)
            .putShort(context.keyEpoch)
            .put(context.direction.byte)
            .put(0)
            .putInt(context.streamId)
            .putInt(sequence)
            .array()
}
