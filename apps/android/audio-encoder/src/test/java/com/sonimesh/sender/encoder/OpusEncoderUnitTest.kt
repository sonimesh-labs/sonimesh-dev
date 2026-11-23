package com.sonimesh.sender.encoder

import java.nio.ByteBuffer
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class OpusEncoderUnitTest {
    @Test
    fun bitrateChoicesUseTheStandardDefaultForUnknownValues() {
        assertEquals(OpusBitrate.LOW, OpusBitrate.fromBitsPerSecond(64_000))
        assertEquals(OpusBitrate.STANDARD, OpusBitrate.fromBitsPerSecond(96_000))
        assertEquals(OpusBitrate.HIGH, OpusBitrate.fromBitsPerSecond(128_000))
        assertEquals(OpusBitrate.STANDARD, OpusBitrate.fromBitsPerSecond(0))
    }

    @Test
    fun inputBackpressureAndEndOfStreamPreserveFrames() {
        val codec = FakeCodec()
        val encoder = OpusEncoder(codec)
        val frame = ShortArray(OpusEncoder.FRAME_SAMPLES)
        frame[0] = 0x1234
        frame[1] = (-2).toShort()
        assertFalse(encoder.tryQueue(frame, 20_000))
        assertEquals(0, codec.queued.size)
        codec.inputIndex = 4
        assertTrue(encoder.tryQueue(frame, 20_000))
        assertArrayEquals(byteArrayOf(0x34, 0x12, 0xfe.toByte(), 0xff.toByte()), codec.input.array().copyOfRange(0, 4))
        assertEquals(Queued(4, 3_840, 20_000, 0), codec.queued.single())
        codec.inputIndex = -1
        assertFalse(encoder.tryQueue(null, 40_000, end = true))
        codec.inputIndex = 5
        assertTrue(encoder.tryQueue(null, 40_000, end = true))
        assertEquals(Queued(5, 0, 40_000, 4), codec.queued.last())
        assertThrows(IllegalStateException::class.java) { encoder.tryQueue(frame, 60_000) }
    }

    @Test
    fun outputIgnoresCodecConfigAndCopiesOnlyPacketRange() {
        val codec = FakeCodec()
        val encoder = OpusEncoder(codec)
        val packets = mutableListOf<ByteArray>()
        assertEquals(OpusEncoder.Output.Empty, encoder.poll(packets::add))
        codec.outputIndex = 2
        codec.offset = 1
        codec.size = 3
        codec.flags = 2 // BUFFER_FLAG_CODEC_CONFIG
        assertEquals(OpusEncoder.Output.Progress, encoder.poll(packets::add))
        assertTrue(packets.isEmpty())
        codec.flags = 4 // BUFFER_FLAG_END_OF_STREAM
        assertEquals(OpusEncoder.Output.End, encoder.poll(packets::add))
        assertArrayEquals(byteArrayOf(10, 11, 12), packets.single())
        assertEquals(listOf(2, 2), codec.released)
    }

    @Test
    fun outputIsReleasedWhenPacketIsOversizedOrConsumerFails() {
        val codec = FakeCodec().apply { outputIndex = 3; size = OpusEncoder.MAX_PACKET_BYTES + 1 }
        val encoder = OpusEncoder(codec)
        assertThrows(IllegalArgumentException::class.java) { encoder.poll {} }
        codec.size = 1
        assertThrows(IllegalStateException::class.java) { encoder.poll { error("consumer failed") } }
        assertEquals(listOf(3, 3), codec.released)
        encoder.close()
        assertTrue(codec.closed)
    }

    private data class Queued(val index: Int, val size: Int, val timeUs: Long, val flags: Int)

    private class FakeCodec : EncoderCodec {
        var inputIndex = -1
        var outputIndex = -1
        val input: ByteBuffer = ByteBuffer.allocate(OpusEncoder.FRAME_SAMPLES * 2)
        val queued = mutableListOf<Queued>()
        val released = mutableListOf<Int>()
        var closed = false
        override var size = 0
        override var offset = 0
        override var flags = 0
        override fun dequeueInput() = inputIndex
        override fun inputBuffer(index: Int) = input
        override fun queueInput(index: Int, size: Int, timeUs: Long, flags: Int) {
            queued.add(Queued(index, size, timeUs, flags))
        }
        override fun dequeueOutput() = outputIndex
        override fun outputBuffer(index: Int) = ByteBuffer.wrap(byteArrayOf(9, 10, 11, 12, 13))
        override fun releaseOutput(index: Int) { released.add(index) }
        override fun release() { closed = true }
    }
}
