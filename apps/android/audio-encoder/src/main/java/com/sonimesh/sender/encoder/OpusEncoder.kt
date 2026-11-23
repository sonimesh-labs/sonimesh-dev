package com.sonimesh.sender.encoder

import android.media.MediaCodec
import android.media.MediaFormat
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** A single-threaded, non-blocking encoder for the controlled audio lab. */
class OpusEncoder internal constructor(private val codec: EncoderCodec) : AutoCloseable {
    private var endSubmitted = false

    /** Null PCM submits an empty end-of-stream; a partial final frame must be padded by the caller. */
    fun tryQueue(pcm: ShortArray?, presentationTimeUs: Long, end: Boolean = false): Boolean {
        check(!endSubmitted) { "end of stream already submitted" }
        require(presentationTimeUs >= 0)
        require((pcm != null && pcm.size == FRAME_SAMPLES) || (pcm == null && end))
        val index = codec.dequeueInput()
        if (index < 0) return false
        val size = if (pcm == null) 0 else {
            val buffer = requireNotNull(codec.inputBuffer(index))
            buffer.clear()
            buffer.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(pcm)
            FRAME_SAMPLES * Short.SIZE_BYTES
        }
        codec.queueInput(index, size, presentationTimeUs, if (end) MediaCodec.BUFFER_FLAG_END_OF_STREAM else 0)
        endSubmitted = end
        return true
    }

    /** Delivers at most one bounded audio packet; codec configuration is not an audio packet. */
    fun poll(onPacket: (ByteArray) -> Unit): Output {
        val index = codec.dequeueOutput()
        if (index == MediaCodec.INFO_TRY_AGAIN_LATER) return Output.Empty
        if (index < 0) return Output.Progress
        try {
            if (codec.size > 0 && codec.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                require(codec.size <= MAX_PACKET_BYTES) { "encoded packet exceeds the lab limit" }
                val packet = ByteArray(codec.size)
                requireNotNull(codec.outputBuffer(index)).apply { position(codec.offset); get(packet) }
                onPacket(packet)
            }
            return if (codec.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) Output.End else Output.Progress
        } finally {
            codec.releaseOutput(index)
        }
    }

    override fun close() = codec.release()

    enum class Output { Empty, Progress, End }

    companion object {
        const val FRAME_SAMPLES = 1_920
        const val MAX_PACKET_BYTES = 1_148

        fun create(): OpusEncoder {
            val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
            try {
                val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
                format.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
                codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                codec.start()
                return OpusEncoder(AndroidEncoderCodec(codec))
            } catch (error: Throwable) {
                try {
                    codec.release()
                } catch (releaseError: Throwable) {
                    error.addSuppressed(releaseError)
                }
                throw error
            }
        }
    }
}

internal interface EncoderCodec {
    val size: Int
    val offset: Int
    val flags: Int
    fun dequeueInput(): Int
    fun inputBuffer(index: Int): ByteBuffer?
    fun queueInput(index: Int, size: Int, timeUs: Long, flags: Int)
    fun dequeueOutput(): Int
    fun outputBuffer(index: Int): ByteBuffer?
    fun releaseOutput(index: Int)
    fun release()
}

private class AndroidEncoderCodec(private val codec: MediaCodec) : EncoderCodec {
    private val info = MediaCodec.BufferInfo()
    override val size get() = info.size
    override val offset get() = info.offset
    override val flags get() = info.flags
    override fun dequeueInput() = codec.dequeueInputBuffer(0)
    override fun inputBuffer(index: Int) = codec.getInputBuffer(index)
    override fun queueInput(index: Int, size: Int, timeUs: Long, flags: Int) =
        codec.queueInputBuffer(index, 0, size, timeUs, flags)
    override fun dequeueOutput() = codec.dequeueOutputBuffer(info, 0)
    override fun outputBuffer(index: Int) = codec.getOutputBuffer(index)
    override fun releaseOutput(index: Int) = codec.releaseOutputBuffer(index, false)
    override fun release() = codec.release()
}
