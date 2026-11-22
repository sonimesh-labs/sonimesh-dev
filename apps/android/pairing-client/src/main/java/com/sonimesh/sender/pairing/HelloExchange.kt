package com.sonimesh.sender.pairing

import com.google.protobuf.CodedInputStream
import com.google.protobuf.CodedOutputStream
import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.InputStream
import java.io.OutputStream

data class NegotiatedHello(val version: Int, val capabilities: List<Int>, val peerName: String)

/** The Hello fields used here are defined by proto/control.proto. */
internal fun exchangeHello(
    input: InputStream,
    output: OutputStream,
    displayName: String,
    capabilities: List<Int>,
): NegotiatedHello {
    require(displayName.toByteArray(Charsets.UTF_8).size <= 128) { "display name too long" }
    require(capabilities.size <= 16 && capabilities.all { it in 1..4 }) { "invalid capabilities" }
    val hello = encodeMessage { coded ->
        coded.writeUInt32(1, 1)
        capabilities.forEach { coded.writeEnum(2, it) }
        coded.writeString(3, displayName)
    }
    val envelope = encodeMessage { coded ->
        coded.writeUInt32(1, 1)
        coded.writeUInt64(2, 1)
        coded.writeByteArray(10, hello)
    }
    DataOutputStream(output).apply { writeInt(envelope.size); write(envelope); flush() }
    val data = DataInputStream(input)
    val length = data.readInt()
    require(length in 1..65_536) { "invalid control frame length" }
    val payload = ByteArray(length)
    data.readFully(payload)
    val peer = decodeHello(payload)
    require(peer.version == 1 && peer.requestId == 1L && 1 in peer.versions) {
        "incompatible desktop hello"
    }
    return NegotiatedHello(1, (1..4).filter { it in capabilities && it in peer.capabilities }, peer.name)
}

internal fun encodeMessage(write: (CodedOutputStream) -> Unit): ByteArray {
    val buffer = ByteArrayOutputStream()
    val coded = CodedOutputStream.newInstance(buffer)
    write(coded)
    coded.flush()
    return buffer.toByteArray()
}

private data class PeerHello(
    val version: Int,
    val requestId: Long,
    val versions: List<Int>,
    val capabilities: List<Int>,
    val name: String,
)

private fun decodeHello(bytes: ByteArray): PeerHello {
    val input = CodedInputStream.newInstance(bytes)
    var version = 0
    var requestId = 0L
    var hello: ByteArray? = null
    while (!input.isAtEnd) {
        when (val tag = input.readTag()) {
            8 -> version = input.readUInt32()
            16 -> requestId = input.readUInt64()
            82 -> { require(hello == null) { "duplicate hello" }; hello = input.readByteArray() }
            90, 98, 106, 114 -> throw IllegalArgumentException("expected hello")
            else -> require(input.skipField(tag)) { "invalid envelope" }
        }
    }
    val fields = CodedInputStream.newInstance(requireNotNull(hello) { "missing hello" })
    val versions = mutableListOf<Int>()
    val capabilities = mutableListOf<Int>()
    var name = ""
    while (!fields.isAtEnd) {
        when (val tag = fields.readTag()) {
            8 -> addBounded(versions, fields.readUInt32(), 8)
            10 -> readPacked(fields, versions, 8)
            16 -> addBounded(capabilities, fields.readEnum(), 16)
            18 -> readPacked(fields, capabilities, 16)
            26 -> {
                name = fields.readStringRequireUtf8()
                require(name.toByteArray(Charsets.UTF_8).size <= 128) { "display name too long" }
            }
            else -> require(fields.skipField(tag)) { "invalid hello" }
        }
    }
    return PeerHello(version, requestId, versions, capabilities, name)
}

private fun readPacked(input: CodedInputStream, values: MutableList<Int>, max: Int) {
    val length = input.readRawVarint32()
    val previous = input.pushLimit(length)
    while (input.bytesUntilLimit > 0) addBounded(values, input.readUInt32(), max)
    input.popLimit(previous)
}

private fun addBounded(values: MutableList<Int>, value: Int, max: Int) {
    require(values.size < max) { "hello field exceeds limit" }
    values.add(value)
}
