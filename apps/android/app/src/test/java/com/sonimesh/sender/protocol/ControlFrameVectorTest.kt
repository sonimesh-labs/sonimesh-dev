package com.sonimesh.sender.protocol

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ControlFrameVectorTest {
    @Test
    fun helloMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = frame(
            message(
                varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
                varintField(ENVELOPE_REQUEST_ID_FIELD, 7u),
                bytesField(
                    ENVELOPE_HELLO_FIELD,
                    message(
                        bytesField(HELLO_SUPPORTED_VERSIONS_FIELD, varint(CONTROL_PROTOCOL_VERSION)),
                        bytesField(HELLO_CAPABILITIES_FIELD, varint(FRAME_DURATION_20_MS)),
                        bytesField(HELLO_DISPLAY_NAME_FIELD, "Lab sender".encodeToByteArray()),
                    ),
                ),
            ),
        )

        assertArrayEquals(EXPECTED_HELLO_FRAME, encoded)

        val envelope = decodeMessage(decodeFrame(EXPECTED_HELLO_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(7uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val hello = decodeMessage(envelope.requiredBytes(ENVELOPE_HELLO_FIELD))
        assertEquals(
            listOf(CONTROL_PROTOCOL_VERSION),
            decodePackedVarints(hello.requiredBytes(HELLO_SUPPORTED_VERSIONS_FIELD)),
        )
        assertEquals(
            listOf(FRAME_DURATION_20_MS),
            decodePackedVarints(hello.requiredBytes(HELLO_CAPABILITIES_FIELD)),
        )
        assertEquals(
            "Lab sender",
            hello.requiredBytes(HELLO_DISPLAY_NAME_FIELD).decodeToString(),
        )
    }

    @Test
    fun epochBearingStartMatchesPublishedFrameAndDecodesExactEpoch() {
        val encoded = startFrame(MEDIA_KEY_EPOCH)

        assertArrayEquals(EXPECTED_START_FRAME, encoded)

        val envelope = decodeMessage(decodeFrame(EXPECTED_START_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(8uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val start = decodeMessage(envelope.requiredBytes(ENVELOPE_START_STREAM_FIELD))
        assertEquals(7uL, start.requiredVarint(START_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, start.requiredVarint(START_STREAM_ID_FIELD))
        assertEquals(48_000uL, start.requiredVarint(START_SAMPLE_RATE_FIELD))
        assertEquals(2uL, start.requiredVarint(START_CHANNELS_FIELD))
        assertEquals(20uL, start.requiredVarint(START_FRAME_DURATION_FIELD))
        assertEquals(MEDIA_KEY_EPOCH, validatedMediaKeyEpoch(start))
    }

    @Test
    fun epochBearingStopMatchesPublishedFrameAndDecodesExactIdentity() {
        val encoded = stopFrame(MEDIA_KEY_EPOCH)

        assertArrayEquals(EXPECTED_STOP_FRAME, encoded)

        val envelope = decodeMessage(decodeFrame(EXPECTED_STOP_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(9uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val stop = decodeMessage(envelope.requiredBytes(ENVELOPE_STOP_STREAM_FIELD))
        assertEquals(7uL, stop.requiredVarint(STOP_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, stop.requiredVarint(STOP_STREAM_ID_FIELD))
        assertEquals(MEDIA_KEY_EPOCH, validatedMediaKeyEpoch(stop, STOP_MEDIA_KEY_EPOCH_FIELD))
    }

    @Test
    fun streamStatusMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = statusFrame(
            mediaSessionId = 7u,
            streamId = 9u,
            state = STREAM_STATE_STREAMING,
        )

        assertArrayEquals(EXPECTED_STATUS_FRAME, encoded)

        val envelope = decodeMessage(decodeFrame(EXPECTED_STATUS_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(10uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val status = decodeMessage(envelope.requiredBytes(ENVELOPE_STREAM_STATUS_FIELD))
        assertEquals(7uL, status.requiredVarint(STATUS_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, status.requiredVarint(STATUS_STREAM_ID_FIELD))
        assertEquals(STREAM_STATE_STREAMING, validatedStreamState(status))
        assertEquals(
            "receiving",
            status.requiredBytes(STATUS_DETAIL_FIELD).decodeToString(),
        )
    }

    @Test
    fun controlErrorMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = errorFrame(ERROR_CODE_RETRY_LATER)

        assertArrayEquals(EXPECTED_ERROR_FRAME, encoded)

        val envelope = decodeMessage(decodeFrame(EXPECTED_ERROR_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(11uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val error = decodeMessage(envelope.requiredBytes(ENVELOPE_ERROR_FIELD))
        assertEquals(ERROR_CODE_RETRY_LATER, validatedErrorCode(error))
        assertEquals(1uL, error.requiredVarint(ERROR_RETRYABLE_FIELD))
        assertEquals(
            "try again",
            error.requiredBytes(ERROR_DETAIL_FIELD).decodeToString(),
        )
    }

    @Test
    fun decoderRejectsInvalidFrameAndSemanticBounds() {
        assertThrows(IllegalArgumentException::class.java) {
            decodeFrame(byteArrayOf(0, 0, 0))
        }

        val mismatchedLength = EXPECTED_START_FRAME.copyOf().also { frame ->
            frame[3] = (frame[3] + 1).toByte()
        }
        assertThrows(IllegalArgumentException::class.java) {
            decodeFrame(mismatchedLength)
        }

        val oversizedLength = ByteBuffer.allocate(FRAME_PREFIX_BYTES)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(MAX_CONTROL_FRAME_BYTES + 1)
            .array()
        assertThrows(IllegalArgumentException::class.java) {
            decodeFrame(oversizedLength)
        }

        val oversizedEpoch = decodeMessage(
            decodeMessage(decodeFrame(startFrame(MAX_MEDIA_KEY_EPOCH + 1u)))
                .requiredBytes(ENVELOPE_START_STREAM_FIELD),
        )
        assertThrows(IllegalArgumentException::class.java) {
            validatedMediaKeyEpoch(oversizedEpoch)
        }

        val oversizedStopEpoch = decodeMessage(
            decodeMessage(decodeFrame(stopFrame(MAX_MEDIA_KEY_EPOCH + 1u)))
                .requiredBytes(ENVELOPE_STOP_STREAM_FIELD),
        )
        assertThrows(IllegalArgumentException::class.java) {
            validatedMediaKeyEpoch(oversizedStopEpoch, STOP_MEDIA_KEY_EPOCH_FIELD)
        }

        val invalidStatuses = listOf(
            statusFrame(0u, 9u, STREAM_STATE_STREAMING),
            statusFrame(7u, 0u, STREAM_STATE_STREAMING),
            statusFrame(7u, 9u, STREAM_STATE_UNSPECIFIED),
            statusFrame(7u, 9u, UNKNOWN_ENUM_VALUE),
        )
        invalidStatuses.forEach { frame ->
            val status = decodeMessage(
                decodeMessage(decodeFrame(frame)).requiredBytes(ENVELOPE_STREAM_STATUS_FIELD),
            )
            assertThrows(IllegalArgumentException::class.java) {
                validatedStreamState(status)
            }
        }

        for (code in listOf(ERROR_CODE_UNSPECIFIED, UNKNOWN_ENUM_VALUE)) {
            val error = decodeMessage(
                decodeMessage(decodeFrame(errorFrame(code))).requiredBytes(ENVELOPE_ERROR_FIELD),
            )
            assertThrows(IllegalArgumentException::class.java) {
                validatedErrorCode(error)
            }
        }
    }

    private fun startFrame(epoch: ULong): ByteArray = frame(
        message(
            varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
            varintField(ENVELOPE_REQUEST_ID_FIELD, 8u),
            bytesField(
                ENVELOPE_START_STREAM_FIELD,
                message(
                    varintField(START_MEDIA_SESSION_ID_FIELD, 7u),
                    varintField(START_STREAM_ID_FIELD, 9u),
                    varintField(START_SAMPLE_RATE_FIELD, 48_000u),
                    varintField(START_CHANNELS_FIELD, 2u),
                    varintField(START_FRAME_DURATION_FIELD, 20u),
                    varintField(START_MEDIA_KEY_EPOCH_FIELD, epoch),
                ),
            ),
        ),
    )

    private fun stopFrame(epoch: ULong): ByteArray = frame(
        message(
            varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
            varintField(ENVELOPE_REQUEST_ID_FIELD, 9u),
            bytesField(
                ENVELOPE_STOP_STREAM_FIELD,
                message(
                    varintField(STOP_MEDIA_SESSION_ID_FIELD, 7u),
                    varintField(STOP_STREAM_ID_FIELD, 9u),
                    varintField(STOP_MEDIA_KEY_EPOCH_FIELD, epoch),
                ),
            ),
        ),
    )

    private fun statusFrame(
        mediaSessionId: ULong,
        streamId: ULong,
        state: ULong,
    ): ByteArray = frame(
        message(
            varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
            varintField(ENVELOPE_REQUEST_ID_FIELD, 10u),
            bytesField(
                ENVELOPE_STREAM_STATUS_FIELD,
                message(
                    varintField(STATUS_MEDIA_SESSION_ID_FIELD, mediaSessionId),
                    varintField(STATUS_STREAM_ID_FIELD, streamId),
                    varintField(STATUS_STATE_FIELD, state),
                    bytesField(STATUS_DETAIL_FIELD, "receiving".encodeToByteArray()),
                ),
            ),
        ),
    )

    private fun errorFrame(code: ULong): ByteArray = frame(
        message(
            varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
            varintField(ENVELOPE_REQUEST_ID_FIELD, 11u),
            bytesField(
                ENVELOPE_ERROR_FIELD,
                message(
                    varintField(ERROR_CODE_FIELD, code),
                    varintField(ERROR_RETRYABLE_FIELD, 1u),
                    bytesField(ERROR_DETAIL_FIELD, "try again".encodeToByteArray()),
                ),
            ),
        ),
    )

    private fun validatedStreamState(message: DecodedMessage): ULong {
        require(message.requiredVarint(STATUS_MEDIA_SESSION_ID_FIELD) != 0uL) {
            "stream status media session ID must be non-zero"
        }
        require(message.requiredVarint(STATUS_STREAM_ID_FIELD) != 0uL) {
            "stream status stream ID must be non-zero"
        }
        val state = message.requiredVarint(STATUS_STATE_FIELD)
        require(state in STREAM_STATE_READY..STREAM_STATE_FAILED) {
            "stream status state is unspecified or unknown"
        }
        return state
    }

    private fun validatedErrorCode(message: DecodedMessage): ULong {
        val code = message.requiredVarint(ERROR_CODE_FIELD)
        require(code in ERROR_CODE_INCOMPATIBLE_VERSION..ERROR_CODE_RETRY_LATER) {
            "control error code is unspecified or unknown"
        }
        return code
    }

    private fun validatedMediaKeyEpoch(
        message: DecodedMessage,
        fieldNumber: Int = START_MEDIA_KEY_EPOCH_FIELD,
    ): ULong {
        val epoch = message.requiredVarint(fieldNumber)
        require(epoch <= MAX_MEDIA_KEY_EPOCH) { "media key epoch exceeds uint16" }
        return epoch
    }

    private fun frame(payload: ByteArray): ByteArray {
        require(payload.size <= MAX_CONTROL_FRAME_BYTES)
        return ByteBuffer.allocate(FRAME_PREFIX_BYTES + payload.size)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(payload.size)
            .put(payload)
            .array()
    }

    private fun decodeFrame(frame: ByteArray): ByteArray {
        require(frame.size >= FRAME_PREFIX_BYTES) { "control frame is truncated" }
        val declaredLength = ByteBuffer.wrap(frame, 0, FRAME_PREFIX_BYTES)
            .order(ByteOrder.BIG_ENDIAN)
            .int
            .toLong() and UINT32_MASK
        require(declaredLength <= MAX_CONTROL_FRAME_BYTES.toLong()) {
            "control frame exceeds the protocol bound"
        }
        require(declaredLength == (frame.size - FRAME_PREFIX_BYTES).toLong()) {
            "control frame length prefix does not match its payload"
        }
        return frame.copyOfRange(FRAME_PREFIX_BYTES, frame.size)
    }

    private fun message(vararg fields: ByteArray): ByteArray {
        val output = ByteArrayOutputStream()
        fields.forEach(output::writeBytes)
        return output.toByteArray()
    }

    private fun varintField(fieldNumber: Int, value: ULong): ByteArray =
        varint(fieldKey(fieldNumber, WIRE_VARINT)) + varint(value)

    private fun bytesField(fieldNumber: Int, value: ByteArray): ByteArray =
        varint(fieldKey(fieldNumber, WIRE_LENGTH_DELIMITED)) +
            varint(value.size.toULong()) +
            value

    private fun fieldKey(fieldNumber: Int, wireType: Int): ULong {
        require(fieldNumber > 0)
        return ((fieldNumber.toULong() shl FIELD_TYPE_BITS) or wireType.toULong())
    }

    private fun varint(value: ULong): ByteArray {
        var remaining = value
        val output = ByteArrayOutputStream(MAX_VARINT_BYTES)
        while (remaining >= VARINT_CONTINUATION.toULong()) {
            output.write(((remaining and VARINT_VALUE_MASK.toULong()).toInt()) or VARINT_CONTINUATION)
            remaining = remaining shr VARINT_VALUE_BITS
        }
        output.write(remaining.toInt())
        return output.toByteArray()
    }

    private fun decodeMessage(payload: ByteArray): DecodedMessage {
        val fields = mutableListOf<DecodedField>()
        var offset = 0
        while (offset < payload.size) {
            val key = readVarint(payload, offset)
            offset = key.nextOffset
            val fieldNumber = (key.value shr FIELD_TYPE_BITS).toInt()
            val wireType = (key.value and FIELD_TYPE_MASK).toInt()
            require(fieldNumber > 0) { "protobuf field number must be non-zero" }
            when (wireType) {
                WIRE_VARINT -> {
                    val decoded = readVarint(payload, offset)
                    fields += DecodedField(fieldNumber, wireType, decoded.value, null)
                    offset = decoded.nextOffset
                }

                WIRE_LENGTH_DELIMITED -> {
                    val length = readVarint(payload, offset)
                    offset = length.nextOffset
                    require(length.value <= Int.MAX_VALUE.toULong()) {
                        "protobuf field length does not fit host storage"
                    }
                    val end = offset.toLong() + length.value.toLong()
                    require(end <= payload.size.toLong()) { "protobuf field is truncated" }
                    fields += DecodedField(
                        fieldNumber,
                        wireType,
                        null,
                        payload.copyOfRange(offset, end.toInt()),
                    )
                    offset = end.toInt()
                }

                else -> throw IllegalArgumentException("unsupported protobuf wire type: $wireType")
            }
        }
        return DecodedMessage(fields)
    }

    private fun decodePackedVarints(payload: ByteArray): List<ULong> {
        val values = mutableListOf<ULong>()
        var offset = 0
        while (offset < payload.size) {
            val decoded = readVarint(payload, offset)
            values += decoded.value
            offset = decoded.nextOffset
        }
        return values
    }

    private fun readVarint(payload: ByteArray, startOffset: Int): DecodedVarint {
        var result = 0uL
        var offset = startOffset
        repeat(MAX_VARINT_BYTES) { index ->
            require(offset < payload.size) { "protobuf varint is truncated" }
            val byte = payload[offset].toInt() and 0xFF
            if (index == MAX_VARINT_BYTES - 1) {
                require(byte <= 1) { "protobuf varint exceeds uint64" }
            }
            result = result or ((byte and VARINT_VALUE_MASK).toULong() shl (index * VARINT_VALUE_BITS))
            offset += 1
            if (byte and VARINT_CONTINUATION == 0) {
                return DecodedVarint(result, offset)
            }
        }
        throw IllegalArgumentException("protobuf varint exceeds uint64")
    }

    private data class DecodedVarint(
        val value: ULong,
        val nextOffset: Int,
    )

    private data class DecodedField(
        val number: Int,
        val wireType: Int,
        val varintValue: ULong?,
        val bytesValue: ByteArray?,
    )

    private class DecodedMessage(private val fields: List<DecodedField>) {
        fun requiredVarint(fieldNumber: Int): ULong {
            val matches = fields.filter { it.number == fieldNumber }
            require(matches.size == 1) { "expected one protobuf field $fieldNumber" }
            val field = matches.single()
            require(field.wireType == WIRE_VARINT && field.varintValue != null) {
                "protobuf field $fieldNumber is not a varint"
            }
            return field.varintValue
        }

        fun requiredBytes(fieldNumber: Int): ByteArray {
            val matches = fields.filter { it.number == fieldNumber }
            require(matches.size == 1) { "expected one protobuf field $fieldNumber" }
            val field = matches.single()
            require(field.wireType == WIRE_LENGTH_DELIMITED && field.bytesValue != null) {
                "protobuf field $fieldNumber is not length-delimited"
            }
            return field.bytesValue
        }
    }

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        const val FRAME_PREFIX_BYTES = 4
        const val MAX_CONTROL_FRAME_BYTES = 64 * 1024
        const val MAX_VARINT_BYTES = 10
        const val FIELD_TYPE_BITS = 3
        const val WIRE_VARINT = 0
        const val WIRE_LENGTH_DELIMITED = 2
        const val VARINT_VALUE_BITS = 7
        const val VARINT_VALUE_MASK = 0x7F
        const val VARINT_CONTINUATION = 0x80
        const val FIELD_TYPE_MASK = 0x07uL
        const val UINT32_MASK = 0xFFFF_FFFFL

        const val ENVELOPE_PROTOCOL_VERSION_FIELD = 1
        const val ENVELOPE_REQUEST_ID_FIELD = 2
        const val ENVELOPE_HELLO_FIELD = 10
        const val ENVELOPE_START_STREAM_FIELD = 11
        const val ENVELOPE_STOP_STREAM_FIELD = 12
        const val ENVELOPE_STREAM_STATUS_FIELD = 13
        const val ENVELOPE_ERROR_FIELD = 14

        const val HELLO_SUPPORTED_VERSIONS_FIELD = 1
        const val HELLO_CAPABILITIES_FIELD = 2
        const val HELLO_DISPLAY_NAME_FIELD = 3

        const val START_MEDIA_SESSION_ID_FIELD = 1
        const val START_STREAM_ID_FIELD = 2
        const val START_SAMPLE_RATE_FIELD = 3
        const val START_CHANNELS_FIELD = 4
        const val START_FRAME_DURATION_FIELD = 5
        const val START_MEDIA_KEY_EPOCH_FIELD = 6

        const val STOP_MEDIA_SESSION_ID_FIELD = 1
        const val STOP_STREAM_ID_FIELD = 2
        const val STOP_MEDIA_KEY_EPOCH_FIELD = 3

        const val STATUS_MEDIA_SESSION_ID_FIELD = 1
        const val STATUS_STREAM_ID_FIELD = 2
        const val STATUS_STATE_FIELD = 3
        const val STATUS_DETAIL_FIELD = 4

        const val ERROR_CODE_FIELD = 1
        const val ERROR_RETRYABLE_FIELD = 2
        const val ERROR_DETAIL_FIELD = 3

        val CONTROL_PROTOCOL_VERSION = 1uL
        val FRAME_DURATION_20_MS = 2uL
        val STREAM_STATE_UNSPECIFIED = 0uL
        val STREAM_STATE_READY = 1uL
        val STREAM_STATE_STREAMING = 2uL
        val STREAM_STATE_FAILED = 5uL
        val ERROR_CODE_UNSPECIFIED = 0uL
        val ERROR_CODE_INCOMPATIBLE_VERSION = 1uL
        val ERROR_CODE_RETRY_LATER = 5uL
        val UNKNOWN_ENUM_VALUE = 99uL
        val MEDIA_KEY_EPOCH = 0x2345uL
        val MAX_MEDIA_KEY_EPOCH = 0xFFFFuL

        val EXPECTED_HELLO_FRAME = hex(
            "000000180801100752120a01011201021a0a4c61622073656e646572",
        )
        val EXPECTED_START_FRAME = hex(
            "00000015080110085a0f080710091880f7022002281430c546",
        )
        val EXPECTED_STOP_FRAME = hex(
            "0000000d0801100962070807100918c546",
        )
        val EXPECTED_STATUS_FRAME = hex(
            "000000170801100a6a110807100918022209726563656976696e67",
        )
        val EXPECTED_ERROR_FRAME = hex(
            "000000150801100b720f080510011a0974727920616761696e",
        )
    }
}
