package com.sonimesh.sender.security

import com.sonimesh.sender.protocol.MEDIA_HEADER_LEN
import com.sonimesh.sender.protocol.MAX_ENCRYPTED_PAYLOAD_LEN
import com.sonimesh.sender.protocol.MediaHeader
import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Cipher
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/** ChaCha20-Poly1305 authentication tag byte length. */
const val POLY1305_TAG_BYTES = 16

/** Largest plaintext accepted before Poly1305 tag expansion. */
const val MAX_MEDIA_PLAINTEXT_LEN = MAX_ENCRYPTED_PAYLOAD_LEN - POLY1305_TAG_BYTES

/**
 * Host-JVM mirror of `sonimesh-protocol`'s `MediaSealer`/`MediaOpener` AEAD
 * packet operations, shared by the vector test so the Android sender and the
 * Rust data plane produce and accept the same authenticated media datagrams.
 * The fixed 36-byte header is the AEAD associated data, and the 96-bit nonce is
 * rebuilt from the packet header's sequence on open.
 */
object MediaCrypto {
    private const val SEQUENCE_OFFSET = 20

    /**
     * Encrypts one bounded payload and authenticates the complete media header.
     *
     * @throws IllegalArgumentException when the plaintext exceeds the hard limit,
     *   mirroring Rust `MediaCryptoError::PayloadTooLarge`.
     */
    fun sealDatagram(
        sessionSecret: ByteArray,
        context: MediaKeyContext,
        flags: Byte,
        sampleTimestamp: Long,
        sequence: Int,
        plaintext: ByteArray,
    ): ByteArray {
        require(plaintext.size <= MAX_MEDIA_PLAINTEXT_LEN) {
            "media payload exceeds the hard limit"
        }
        val header = MediaHeader(
            flags = flags,
            sessionId = context.sessionId,
            streamId = context.streamId,
            sequence = sequence,
            sampleTimestamp = sampleTimestamp,
            keyEpoch = context.keyEpoch,
        ).encode(plaintext.size + POLY1305_TAG_BYTES)
        val ciphertext = transform(
            mode = Cipher.ENCRYPT_MODE,
            key = MediaKey.deriveMediaKey(sessionSecret, context),
            nonce = MediaKey.mediaNonce(context, sequence),
            associatedData = header,
            input = plaintext,
        )
        return header + ciphertext
    }

    /**
     * Authenticates and decrypts one datagram into its plaintext.
     *
     * @throws IllegalArgumentException when the datagram cannot contain a header
     *   plus authentication tag, mirroring Rust `MediaCryptoError::TruncatedTag`.
     * @throws GeneralSecurityException when authentication fails.
     */
    fun openDatagram(
        sessionSecret: ByteArray,
        context: MediaKeyContext,
        datagram: ByteArray,
    ): ByteArray {
        require(datagram.size >= MEDIA_HEADER_LEN + POLY1305_TAG_BYTES) {
            "media datagram is truncated"
        }
        val header = datagram.copyOfRange(0, MEDIA_HEADER_LEN)
        val ciphertext = datagram.copyOfRange(MEDIA_HEADER_LEN, datagram.size)
        val sequence = intAt(header, SEQUENCE_OFFSET)
        return transform(
            mode = Cipher.DECRYPT_MODE,
            key = MediaKey.deriveMediaKey(sessionSecret, context),
            nonce = MediaKey.mediaNonce(context, sequence),
            associatedData = header,
            input = ciphertext,
        )
    }

    private fun transform(
        mode: Int,
        key: ByteArray,
        nonce: ByteArray,
        associatedData: ByteArray,
        input: ByteArray,
    ): ByteArray {
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(mode, SecretKeySpec(key, "ChaCha20"), IvParameterSpec(nonce))
        cipher.updateAAD(associatedData)
        return cipher.doFinal(input)
    }

    private fun intAt(bytes: ByteArray, offset: Int): Int =
        ByteBuffer.wrap(bytes, offset, 4).order(ByteOrder.BIG_ENDIAN).int
}
