package com.sonimesh.opusspike

import java.nio.ByteBuffer
import java.nio.ByteOrder
import javax.crypto.Cipher
import javax.crypto.Mac
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec

/** Lab-only sender for ADR 0005's Android-to-desktop packet shape. */
internal class LabMediaSealer(secret: ByteArray, private val session: Long, private val stream: Int, private val epoch: Short) {
    private val key: ByteArray

    init {
        require(secret.size == 32 && session != 0L && stream != 0)
        val extract = Mac.getInstance("HmacSHA256")
        extract.init(SecretKeySpec("sonimesh media root v1".toByteArray(Charsets.US_ASCII), "HmacSHA256"))
        val root = extract.doFinal(secret)
        val label = "sonimesh media stream key v1".toByteArray(Charsets.US_ASCII)
        val info = ByteBuffer.allocate(label.size + 17).order(ByteOrder.BIG_ENDIAN)
            .put(label).put(1).putLong(session).putInt(stream).putShort(epoch).put(0).put(1).array()
        val expand = Mac.getInstance("HmacSHA256")
        expand.init(SecretKeySpec(root, "HmacSHA256"))
        key = expand.doFinal(info)
        root.fill(0)
    }

    fun seal(sequence: Long, timestamp: Long, flags: Byte, plaintext: ByteArray): ByteArray {
        require(sequence in 0..0xffff_ffffL && plaintext.isNotEmpty() && plaintext.size <= 1_148)
        val header = ByteBuffer.allocate(36).order(ByteOrder.BIG_ENDIAN)
            .putInt(0x534e4d53).put(1).put(flags).putShort(36).putLong(session).putInt(stream)
            .putInt(sequence.toInt()).putLong(timestamp).putShort((plaintext.size + 16).toShort()).putShort(epoch).array()
        val nonce = ByteBuffer.allocate(12).order(ByteOrder.BIG_ENDIAN)
            .putShort(epoch).put(0).put(0).putInt(stream).putInt(sequence.toInt()).array()
        val cipher = Cipher.getInstance("ChaCha20-Poly1305")
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, "ChaCha20"), IvParameterSpec(nonce))
        cipher.updateAAD(header)
        return header + cipher.doFinal(plaintext)
    }

    fun clear() { key.fill(0) }
}
