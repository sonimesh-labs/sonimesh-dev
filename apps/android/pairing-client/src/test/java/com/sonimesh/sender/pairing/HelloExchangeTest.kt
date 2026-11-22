package com.sonimesh.sender.pairing

import com.google.protobuf.CodedOutputStream
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class HelloExchangeTest {
    @Test
    fun negotiates_packed_and_unpacked_capabilities() {
        val output = ByteArrayOutputStream()
        val result = exchangeHello(ByteArrayInputStream(response()), output, "android", listOf(1, 2, 4))
        assertEquals(listOf(2, 4), result.capabilities)
        assertEquals("desktop", result.peerName)
        assertEquals(1, result.version)
    }

    @Test
    fun rejects_wrong_request_and_oversized_frames() {
        assertThrows(IllegalArgumentException::class.java) {
            exchangeHello(ByteArrayInputStream(response(request = 2)), ByteArrayOutputStream(), "", listOf(2))
        }
        val oversized = ByteArrayOutputStream().also { DataOutputStream(it).writeInt(65_537) }.toByteArray()
        assertThrows(IllegalArgumentException::class.java) {
            exchangeHello(ByteArrayInputStream(oversized), ByteArrayOutputStream(), "", listOf(2))
        }
    }

    private fun response(request: Long = 1): ByteArray {
        val hello = message {
            it.writeByteArray(1, byteArrayOf(1))
            it.writeByteArray(2, byteArrayOf(2, 4, 99))
            it.writeEnum(2, 2)
            it.writeString(3, "desktop")
        }
        val envelope = message {
            it.writeUInt32(1, 1)
            it.writeUInt64(2, request)
            it.writeByteArray(10, hello)
        }
        return ByteArrayOutputStream().also { DataOutputStream(it).apply { writeInt(envelope.size); write(envelope) } }.toByteArray()
    }

    private fun message(write: (CodedOutputStream) -> Unit): ByteArray {
        val output = ByteArrayOutputStream()
        val coded = CodedOutputStream.newInstance(output)
        write(coded)
        coded.flush()
        return output.toByteArray()
    }
}
