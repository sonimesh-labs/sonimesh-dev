package com.sonimesh.sender.protocol

import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Four-byte big-endian length prefix before every version-one control payload. */
const val FRAME_PREFIX_BYTES = 4

/** Largest length-prefixed Protobuf control payload. */
const val MAX_CONTROL_FRAME_BYTES = 64 * 1024

/** Exact control protocol version accepted by the controlled-lab implementation. */
val CONTROL_PROTOCOL_VERSION = 1uL

/** Wire value of the 20 ms frame-duration hello capability. */
val FRAME_DURATION_20_MS = 2uL

/** Unspecified stream state, rejected by version-one validation. */
val STREAM_STATE_UNSPECIFIED = 0uL

/** Stream state: ready to receive media. */
val STREAM_STATE_READY = 1uL

/** Stream state: actively receiving media. */
val STREAM_STATE_STREAMING = 2uL

/** Stream state: failed, the highest valid version-one state. */
val STREAM_STATE_FAILED = 5uL

/** Unspecified error code, rejected by version-one validation. */
val ERROR_CODE_UNSPECIFIED = 0uL

/** Error code: incompatible protocol version. */
val ERROR_CODE_INCOMPATIBLE_VERSION = 1uL

/** Error code: retry later, the highest valid version-one code. */
val ERROR_CODE_RETRY_LATER = 5uL

/** Largest wire epoch representable by the version-one media key context. */
val MAX_MEDIA_KEY_EPOCH = 0xFFFFuL

// Envelope field numbers from proto/control.proto.
const val ENVELOPE_PROTOCOL_VERSION_FIELD = 1
const val ENVELOPE_REQUEST_ID_FIELD = 2
const val ENVELOPE_HELLO_FIELD = 10
const val ENVELOPE_START_STREAM_FIELD = 11
const val ENVELOPE_STOP_STREAM_FIELD = 12
const val ENVELOPE_STREAM_STATUS_FIELD = 13
const val ENVELOPE_ERROR_FIELD = 14

// Hello field numbers from proto/control.proto.
const val HELLO_SUPPORTED_VERSIONS_FIELD = 1
const val HELLO_CAPABILITIES_FIELD = 2
const val HELLO_DISPLAY_NAME_FIELD = 3

// StartStream field numbers from proto/control.proto.
const val START_MEDIA_SESSION_ID_FIELD = 1
const val START_STREAM_ID_FIELD = 2
const val START_SAMPLE_RATE_FIELD = 3
const val START_CHANNELS_FIELD = 4
const val START_FRAME_DURATION_FIELD = 5
const val START_MEDIA_KEY_EPOCH_FIELD = 6

// StopStream field numbers from proto/control.proto.
const val STOP_MEDIA_SESSION_ID_FIELD = 1
const val STOP_STREAM_ID_FIELD = 2
const val STOP_MEDIA_KEY_EPOCH_FIELD = 3

// StreamStatus field numbers from proto/control.proto.
const val STATUS_MEDIA_SESSION_ID_FIELD = 1
const val STATUS_STREAM_ID_FIELD = 2
const val STATUS_STATE_FIELD = 3
const val STATUS_DETAIL_FIELD = 4

// ControlError field numbers from proto/control.proto.
const val ERROR_CODE_FIELD = 1
const val ERROR_RETRYABLE_FIELD = 2
const val ERROR_DETAIL_FIELD = 3

/**
 * Host-JVM mirror of `sonimesh-protocol`'s version-one control-frame wire
 * codec, shared by the control-frame vector test so the Android sender and the
 * Rust control plane frame the same big-endian length-prefixed Protobuf
 * payloads. Builders emit raw wire bytes; the `validated*` helpers enforce the
 * semantic bounds of Rust `validate_control_envelope`.
 */
object ControlFrame {
    private const val MAX_VARINT_BYTES = 10
    private const val FIELD_TYPE_BITS = 3
    private const val WIRE_VARINT = 0
    private const val WIRE_LENGTH_DELIMITED = 2
    private const val VARINT_VALUE_BITS = 7
    private const val VARINT_VALUE_MASK = 0x7F
    private const val VARINT_CONTINUATION = 0x80
    private const val FIELD_TYPE_MASK = 0x07uL
    private const val UINT32_MASK = 0xFFFF_FFFFL

    /** Encodes a version-one Hello envelope as a length-prefixed frame. */
    fun helloFrame(
        requestId: ULong,
        supportedVersions: List<ULong>,
        capabilities: List<ULong>,
        displayName: String,
    ): ByteArray = frame(
        message(
            varintField(ENVELOPE_PROTOCOL_VERSION_FIELD, CONTROL_PROTOCOL_VERSION),
            varintField(ENVELOPE_REQUEST_ID_FIELD, requestId),
            bytesField(
                ENVELOPE_HELLO_FIELD,
                message(
                    bytesField(HELLO_SUPPORTED_VERSIONS_FIELD, packedVarints(supportedVersions)),
                    bytesField(HELLO_CAPABILITIES_FIELD, packedVarints(capabilities)),
                    bytesField(HELLO_DISPLAY_NAME_FIELD, displayName.encodeToByteArray()),
                ),
            ),
        ),
    )

    /** Encodes the published epoch-bearing StartStream envelope as a frame. */
    fun startFrame(epoch: ULong): ByteArray = frame(
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

    /** Encodes the published epoch-bearing StopStream envelope as a frame. */
    fun stopFrame(epoch: ULong): ByteArray = frame(
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

    /** Encodes a version-one StreamStatus envelope as a length-prefixed frame. */
    fun statusFrame(
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

    /** Encodes a version-one ControlError envelope as a length-prefixed frame. */
    fun errorFrame(code: ULong): ByteArray = frame(
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

    /**
     * Validates a StreamStatus message's identity and state bounds.
     *
     * @throws IllegalArgumentException when the identity is zero or the state
     *   is unspecified or unknown, mirroring Rust `validate_status`.
     */
    fun validatedStreamState(message: DecodedMessage): ULong {
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

    /**
     * Validates a ControlError message's code bound.
     *
     * @throws IllegalArgumentException when the code is unspecified or unknown,
     *   mirroring Rust `validate_error`.
     */
    fun validatedErrorCode(message: DecodedMessage): ULong {
        val code = message.requiredVarint(ERROR_CODE_FIELD)
        require(code in ERROR_CODE_INCOMPATIBLE_VERSION..ERROR_CODE_RETRY_LATER) {
            "control error code is unspecified or unknown"
        }
        return code
    }

    /**
     * Validates a media key epoch field's uint16 bound.
     *
     * @throws IllegalArgumentException when the epoch exceeds the media
     *   protocol's 16-bit field, mirroring Rust `validate_media_key_epoch`.
     */
    fun validatedMediaKeyEpoch(
        message: DecodedMessage,
        fieldNumber: Int = START_MEDIA_KEY_EPOCH_FIELD,
    ): ULong {
        val epoch = message.requiredVarint(fieldNumber)
        require(epoch <= MAX_MEDIA_KEY_EPOCH) { "media key epoch exceeds uint16" }
        return epoch
    }

    /** Strips and verifies the big-endian length prefix of one control frame. */
    fun decodeFrame(frame: ByteArray): ByteArray {
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

    /** Decodes one length-prefixed Protobuf message into its wire fields. */
    fun decodeMessage(payload: ByteArray): DecodedMessage {
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

    /** Decodes a packed repeated varint payload into its values. */
    fun decodePackedVarints(payload: ByteArray): List<ULong> {
        val values = mutableListOf<ULong>()
        var offset = 0
        while (offset < payload.size) {
            val decoded = readVarint(payload, offset)
            values += decoded.value
            offset = decoded.nextOffset
        }
        return values
    }

    private fun frame(payload: ByteArray): ByteArray {
        require(payload.size <= MAX_CONTROL_FRAME_BYTES)
        return ByteBuffer.allocate(FRAME_PREFIX_BYTES + payload.size)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(payload.size)
            .put(payload)
            .array()
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

    private fun packedVarints(values: List<ULong>): ByteArray =
        values.flatMap { varint(it).toList() }.toByteArray()

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

    data class DecodedVarint(
        val value: ULong,
        val nextOffset: Int,
    )

    data class DecodedField(
        val number: Int,
        val wireType: Int,
        val varintValue: ULong?,
        val bytesValue: ByteArray?,
    )

    class DecodedMessage(private val fields: List<DecodedField>) {
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
}
