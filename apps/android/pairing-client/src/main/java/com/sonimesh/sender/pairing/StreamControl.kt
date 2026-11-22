package com.sonimesh.sender.pairing

import com.google.protobuf.CodedInputStream
import com.google.protobuf.CodedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.io.OutputStream

/** A uint64 session ID is represented by its bit pattern in [sessionId]. */
data class StreamIdentity(val sessionId: Long, val streamId: Long, val keyEpoch: Int) {
    internal fun validate() {
        require(sessionId != 0L && streamId in 1..0xffff_ffffL && keyEpoch in 0..65_535) {
            "invalid stream identity"
        }
    }
}

data class StreamParameters(
    val identity: StreamIdentity,
    val sampleRateHz: Int,
    val channels: Int,
    val frameDurationMs: Int,
) {
    internal fun validate() {
        identity.validate()
        require(sampleRateHz in 8_000..192_000 && channels in 1..8 && frameDurationMs in listOf(10, 20)) {
            "invalid stream parameters"
        }
    }
}

sealed class StreamControlResult {
    /** The status wire format echoes the session and stream IDs, but not the key epoch. */
    data class Status(val identity: StreamIdentity, val state: Int, val detail: String) : StreamControlResult()
    data class Rejected(val code: Int, val retryable: Boolean, val detail: String) : StreamControlResult()
}

internal fun exchangeStreamCommand(
    input: InputStream,
    output: OutputStream,
    requestId: Long,
    identity: StreamIdentity,
    field: Int,
    write: (CodedOutputStream) -> Unit,
): StreamControlResult {
    val payload = encodeMessage(write)
    val envelope = encodeMessage {
        it.writeUInt32(1, 1)
        it.writeUInt64(2, requestId)
        it.writeByteArray(field, payload)
    }
    DataOutputStream(output).apply { writeInt(envelope.size); write(envelope); flush() }
    val framed = DataInputStream(input)
    val length = framed.readInt()
    require(length in 1..65_536) { "invalid control frame length" }
    val response = ByteArray(length)
    framed.readFully(response)
    val coded = CodedInputStream.newInstance(response)
    var version = 0
    var responseId = 0L
    var kind = 0
    var body: ByteArray? = null
    while (!coded.isAtEnd) {
        when (val tag = coded.readTag()) {
            8 -> version = coded.readUInt32()
            16 -> responseId = coded.readUInt64()
            82, 90, 98, 106, 114 -> {
                require(body == null && tag in listOf(106, 114)) { "unexpected control response" }
                kind = tag
                body = coded.readByteArray()
            }
            else -> require(coded.skipField(tag)) { "invalid control envelope" }
        }
    }
    require(version == 1 && responseId == requestId) { "mismatched control response" }
    val fields = CodedInputStream.newInstance(requireNotNull(body) { "missing control response" })
    var first = 0L
    var second = 0L
    var third = 0
    var detail = ""
    while (!fields.isAtEnd) {
        when (val tag = fields.readTag()) {
            8 -> first = if (kind == 106) fields.readUInt64() else fields.readEnum().toLong()
            16 -> second = if (kind == 106) fields.readUInt32().toLong() and 0xffff_ffffL else if (fields.readBool()) 1 else 0
            24 -> if (kind == 106) third = fields.readEnum() else require(fields.skipField(tag))
            26 -> if (kind == 114) detail = fields.readStringRequireUtf8() else require(fields.skipField(tag))
            34 -> if (kind == 106) detail = fields.readStringRequireUtf8() else require(fields.skipField(tag))
            else -> require(fields.skipField(tag)) { "invalid control response" }
        }
    }
    require(detail.toByteArray(Charsets.UTF_8).size <= 512) { "control detail too long" }
    return if (kind == 106) {
        require(first == identity.sessionId && second == identity.streamId && third in 1..5) {
            "invalid stream status"
        }
        StreamControlResult.Status(identity, third, detail)
    } else {
        require(first in 1..5) { "invalid control error" }
        StreamControlResult.Rejected(first.toInt(), second != 0L, detail)
    }
}
