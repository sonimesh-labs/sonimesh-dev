package com.sonimesh.sender.pairing

import com.google.protobuf.CodedInputStream
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class StreamControlTest {
    private val identity = StreamIdentity(-1, 0xffff_ffffL, 7)

    @Test
    fun start_request_and_status_preserve_unsigned_identities() {
        val output = ByteArrayOutputStream()
        val result = exchangeStreamCommand(ByteArrayInputStream(status()), output, 2, identity, 11) {
            it.writeUInt64(1, identity.sessionId)
            it.writeUInt32(2, identity.streamId.toInt())
            it.writeUInt32(3, 48_000)
            it.writeUInt32(4, 2)
            it.writeUInt32(5, 20)
            it.writeUInt32(6, identity.keyEpoch)
        }
        assertEquals(StreamControlResult.Status(identity, 2, "receiving"), result)
        val framed = DataInputStream(ByteArrayInputStream(output.toByteArray()))
        val bytes = ByteArray(framed.readInt()).also { framed.readFully(it) }
        val envelope = CodedInputStream.newInstance(bytes)
        assertEquals(8, envelope.readTag()); assertEquals(1, envelope.readUInt32())
        assertEquals(16, envelope.readTag()); assertEquals(2L, envelope.readUInt64())
        assertEquals(90, envelope.readTag())
        val start = CodedInputStream.newInstance(envelope.readByteArray())
        assertEquals(8, start.readTag()); assertEquals(-1L, start.readUInt64())
        assertEquals(16, start.readTag()); assertEquals(-1, start.readUInt32())
        assertEquals(24, start.readTag()); assertEquals(48_000, start.readUInt32())
    }

    @Test
    fun structured_rejection_is_returned_to_caller() {
        val error = encodeMessage { it.writeEnum(1, 5); it.writeBool(2, true); it.writeString(3, "busy") }
        assertEquals(
            StreamControlResult.Rejected(5, true, "busy"),
            exchangeStreamCommand(ByteArrayInputStream(reply(2, 14, error)), ByteArrayOutputStream(), 2, identity, 12) {},
        )
    }

    @Test
    fun rejects_mismatched_responses_and_invalid_parameters() {
        for (response in listOf(status(request = 3), status(session = 42), status(state = 99), reply(2, 14, byteArrayOf()))) {
            assertThrows(IllegalArgumentException::class.java) {
                exchangeStreamCommand(ByteArrayInputStream(response), ByteArrayOutputStream(), 2, identity, 12) {}
            }
        }
        assertThrows(IllegalArgumentException::class.java) { StreamIdentity(1, 0, 0).validate() }
        assertThrows(IllegalArgumentException::class.java) { StreamParameters(identity, 48_000, 2, 15).validate() }
    }

    private fun status(request: Long = 2, session: Long = -1, state: Int = 2): ByteArray {
        val body = encodeMessage {
            it.writeUInt64(1, session); it.writeUInt32(2, -1); it.writeEnum(3, state); it.writeString(4, "receiving")
        }
        return reply(request, 13, body)
    }

    private fun reply(request: Long, field: Int, body: ByteArray): ByteArray {
        val envelope = encodeMessage { it.writeUInt32(1, 1); it.writeUInt64(2, request); it.writeByteArray(field, body) }
        return ByteArrayOutputStream().also { DataOutputStream(it).apply { writeInt(envelope.size); write(envelope) } }.toByteArray()
    }
}
