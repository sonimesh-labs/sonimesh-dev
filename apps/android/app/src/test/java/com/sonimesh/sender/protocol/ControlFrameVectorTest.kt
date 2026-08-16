package com.sonimesh.sender.protocol

import java.nio.ByteBuffer
import java.nio.ByteOrder
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ControlFrameVectorTest {
    @Test
    fun helloMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = ControlFrame.helloFrame(
            requestId = 7u,
            supportedVersions = listOf(CONTROL_PROTOCOL_VERSION),
            capabilities = listOf(FRAME_DURATION_20_MS),
            displayName = "Lab sender",
        )

        assertArrayEquals(EXPECTED_HELLO_FRAME, encoded)

        val envelope = ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_HELLO_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(7uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val hello = ControlFrame.decodeMessage(envelope.requiredBytes(ENVELOPE_HELLO_FIELD))
        assertEquals(
            listOf(CONTROL_PROTOCOL_VERSION),
            ControlFrame.decodePackedVarints(hello.requiredBytes(HELLO_SUPPORTED_VERSIONS_FIELD)),
        )
        assertEquals(
            listOf(FRAME_DURATION_20_MS),
            ControlFrame.decodePackedVarints(hello.requiredBytes(HELLO_CAPABILITIES_FIELD)),
        )
        assertEquals(
            "Lab sender",
            hello.requiredBytes(HELLO_DISPLAY_NAME_FIELD).decodeToString(),
        )
    }

    @Test
    fun publishedHelloFrameDecodesAndNegotiatesCanonicalCapabilities() {
        val hello = ControlFrame.decodeMessage(
            ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_HELLO_FRAME))
                .requiredBytes(ENVELOPE_HELLO_FIELD),
        )
        val negotiated = HelloNegotiation.negotiateHello(
            peerSupportedVersions = ControlFrame.decodePackedVarints(
                hello.requiredBytes(HELLO_SUPPORTED_VERSIONS_FIELD),
            ).map { it.toLong() },
            peerCapabilities = ControlFrame.decodePackedVarints(
                hello.requiredBytes(HELLO_CAPABILITIES_FIELD),
            ).map { it.toLong() },
            peerDisplayName = hello.requiredBytes(HELLO_DISPLAY_NAME_FIELD).decodeToString(),
            localCapabilities = listOf(HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS),
        )

        assertEquals(HelloNegotiation.CONTROL_PROTOCOL_VERSION, negotiated.version)
        assertEquals(
            listOf(HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS),
            negotiated.capabilities,
        )
    }

    @Test
    fun richHelloFrameDecodesAndNegotiatesCanonicalIntersectionIgnoringUnknown() {
        val encoded = ControlFrame.helloFrame(
            requestId = 7u,
            supportedVersions = listOf(
                99uL,
                CONTROL_PROTOCOL_VERSION,
                CONTROL_PROTOCOL_VERSION,
            ),
            capabilities = listOf(
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS.toULong(),
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS.toULong(),
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS.toULong(),
                9_999uL,
            ),
            displayName = "Lab sender",
        )

        val hello = ControlFrame.decodeMessage(
            ControlFrame.decodeMessage(ControlFrame.decodeFrame(encoded))
                .requiredBytes(ENVELOPE_HELLO_FIELD),
        )
        val negotiated = HelloNegotiation.negotiateHello(
            peerSupportedVersions = ControlFrame.decodePackedVarints(
                hello.requiredBytes(HELLO_SUPPORTED_VERSIONS_FIELD),
            ).map { it.toLong() },
            peerCapabilities = ControlFrame.decodePackedVarints(
                hello.requiredBytes(HELLO_CAPABILITIES_FIELD),
            ).map { it.toLong() },
            peerDisplayName = hello.requiredBytes(HELLO_DISPLAY_NAME_FIELD).decodeToString(),
            localCapabilities = listOf(
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS,
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,
                HelloNegotiation.CAPABILITY_MANUAL_ADDRESS,
            ),
        )

        assertEquals(HelloNegotiation.CONTROL_PROTOCOL_VERSION, negotiated.version)
        assertEquals(
            listOf(
                HelloNegotiation.CAPABILITY_FRAME_DURATION_20_MS,
                HelloNegotiation.CAPABILITY_NETWORK_DIAGNOSTICS,
            ),
            negotiated.capabilities,
        )
    }

    @Test
    fun epochBearingStartMatchesPublishedFrameAndDecodesExactEpoch() {
        val encoded = ControlFrame.startFrame(MEDIA_KEY_EPOCH)

        assertArrayEquals(EXPECTED_START_FRAME, encoded)

        val envelope = ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_START_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(8uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val start = ControlFrame.decodeMessage(envelope.requiredBytes(ENVELOPE_START_STREAM_FIELD))
        assertEquals(7uL, start.requiredVarint(START_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, start.requiredVarint(START_STREAM_ID_FIELD))
        assertEquals(48_000uL, start.requiredVarint(START_SAMPLE_RATE_FIELD))
        assertEquals(2uL, start.requiredVarint(START_CHANNELS_FIELD))
        assertEquals(20uL, start.requiredVarint(START_FRAME_DURATION_FIELD))
        assertEquals(MEDIA_KEY_EPOCH, ControlFrame.validatedMediaKeyEpoch(start))
    }

    @Test
    fun epochBearingStopMatchesPublishedFrameAndDecodesExactIdentity() {
        val encoded = ControlFrame.stopFrame(MEDIA_KEY_EPOCH)

        assertArrayEquals(EXPECTED_STOP_FRAME, encoded)

        val envelope = ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_STOP_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(9uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val stop = ControlFrame.decodeMessage(envelope.requiredBytes(ENVELOPE_STOP_STREAM_FIELD))
        assertEquals(7uL, stop.requiredVarint(STOP_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, stop.requiredVarint(STOP_STREAM_ID_FIELD))
        assertEquals(MEDIA_KEY_EPOCH, ControlFrame.validatedMediaKeyEpoch(stop, STOP_MEDIA_KEY_EPOCH_FIELD))
    }

    @Test
    fun streamStatusMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = ControlFrame.statusFrame(
            mediaSessionId = 7u,
            streamId = 9u,
            state = STREAM_STATE_STREAMING,
        )

        assertArrayEquals(EXPECTED_STATUS_FRAME, encoded)

        val envelope = ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_STATUS_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(10uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val status = ControlFrame.decodeMessage(envelope.requiredBytes(ENVELOPE_STREAM_STATUS_FIELD))
        assertEquals(7uL, status.requiredVarint(STATUS_MEDIA_SESSION_ID_FIELD))
        assertEquals(9uL, status.requiredVarint(STATUS_STREAM_ID_FIELD))
        assertEquals(STREAM_STATE_STREAMING, ControlFrame.validatedStreamState(status))
        assertEquals(
            "receiving",
            status.requiredBytes(STATUS_DETAIL_FIELD).decodeToString(),
        )
    }

    @Test
    fun controlErrorMatchesPublishedFrameAndDecodesSemanticFields() {
        val encoded = ControlFrame.errorFrame(ERROR_CODE_RETRY_LATER)

        assertArrayEquals(EXPECTED_ERROR_FRAME, encoded)

        val envelope = ControlFrame.decodeMessage(ControlFrame.decodeFrame(EXPECTED_ERROR_FRAME))
        assertEquals(CONTROL_PROTOCOL_VERSION, envelope.requiredVarint(ENVELOPE_PROTOCOL_VERSION_FIELD))
        assertEquals(11uL, envelope.requiredVarint(ENVELOPE_REQUEST_ID_FIELD))
        val error = ControlFrame.decodeMessage(envelope.requiredBytes(ENVELOPE_ERROR_FIELD))
        assertEquals(ERROR_CODE_RETRY_LATER, ControlFrame.validatedErrorCode(error))
        assertEquals(1uL, error.requiredVarint(ERROR_RETRYABLE_FIELD))
        assertEquals(
            "try again",
            error.requiredBytes(ERROR_DETAIL_FIELD).decodeToString(),
        )
    }

    @Test
    fun decoderRejectsInvalidFrameAndSemanticBounds() {
        assertThrows(IllegalArgumentException::class.java) {
            ControlFrame.decodeFrame(byteArrayOf(0, 0, 0))
        }

        val mismatchedLength = EXPECTED_START_FRAME.copyOf().also { frame ->
            frame[3] = (frame[3] + 1).toByte()
        }
        assertThrows(IllegalArgumentException::class.java) {
            ControlFrame.decodeFrame(mismatchedLength)
        }

        val oversizedLength = ByteBuffer.allocate(FRAME_PREFIX_BYTES)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(MAX_CONTROL_FRAME_BYTES + 1)
            .array()
        assertThrows(IllegalArgumentException::class.java) {
            ControlFrame.decodeFrame(oversizedLength)
        }

        val oversizedEpoch = ControlFrame.decodeMessage(
            ControlFrame.decodeMessage(
                ControlFrame.decodeFrame(ControlFrame.startFrame(MAX_MEDIA_KEY_EPOCH + 1u)),
            ).requiredBytes(ENVELOPE_START_STREAM_FIELD),
        )
        assertThrows(IllegalArgumentException::class.java) {
            ControlFrame.validatedMediaKeyEpoch(oversizedEpoch)
        }

        val oversizedStopEpoch = ControlFrame.decodeMessage(
            ControlFrame.decodeMessage(
                ControlFrame.decodeFrame(ControlFrame.stopFrame(MAX_MEDIA_KEY_EPOCH + 1u)),
            ).requiredBytes(ENVELOPE_STOP_STREAM_FIELD),
        )
        assertThrows(IllegalArgumentException::class.java) {
            ControlFrame.validatedMediaKeyEpoch(oversizedStopEpoch, STOP_MEDIA_KEY_EPOCH_FIELD)
        }

        val invalidStatuses = listOf(
            ControlFrame.statusFrame(0u, 9u, STREAM_STATE_STREAMING),
            ControlFrame.statusFrame(7u, 0u, STREAM_STATE_STREAMING),
            ControlFrame.statusFrame(7u, 9u, STREAM_STATE_UNSPECIFIED),
            ControlFrame.statusFrame(7u, 9u, UNKNOWN_ENUM_VALUE),
        )
        invalidStatuses.forEach { frame ->
            val status = ControlFrame.decodeMessage(
                ControlFrame.decodeMessage(ControlFrame.decodeFrame(frame))
                    .requiredBytes(ENVELOPE_STREAM_STATUS_FIELD),
            )
            assertThrows(IllegalArgumentException::class.java) {
                ControlFrame.validatedStreamState(status)
            }
        }

        for (code in listOf(ERROR_CODE_UNSPECIFIED, UNKNOWN_ENUM_VALUE)) {
            val error = ControlFrame.decodeMessage(
                ControlFrame.decodeMessage(
                    ControlFrame.decodeFrame(ControlFrame.errorFrame(code)),
                ).requiredBytes(ENVELOPE_ERROR_FIELD),
            )
            assertThrows(IllegalArgumentException::class.java) {
                ControlFrame.validatedErrorCode(error)
            }
        }
    }

    @Test
    fun pinsTheControlFrameBounds() {
        assertEquals(4, FRAME_PREFIX_BYTES)
        assertEquals(64 * 1024, MAX_CONTROL_FRAME_BYTES)
    }

    private companion object {
        private fun hex(value: String): ByteArray {
            require(value.length % 2 == 0)
            return ByteArray(value.length / 2) { index ->
                value.substring(index * 2, index * 2 + 2).toInt(16).toByte()
            }
        }

        val UNKNOWN_ENUM_VALUE = 99uL
        val MEDIA_KEY_EPOCH = 0x2345uL

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
