package com.sonimesh.sender.encoder

import android.media.MediaCodec
import android.media.MediaFormat
import java.nio.ByteOrder

/** A single-threaded, non-blocking encoder for the controlled audio lab. */
class OpusEncoder private constructor(private val codec: MediaCodec) : AutoCloseable {
    private val info = MediaCodec.BufferInfo()
    private var endSubmitted = false

    /** Null PCM submits an empty end-of-stream; a partial final frame must be padded by the caller. */
    fun tryQueue(pcm: ShortArray?, presentationTimeUs: Long, end: Boolean = false): Boolean {
        check(!endSubmitted) { "end of stream already submitted" }
        require(presentationTimeUs >= 0)
        require((pcm != null && pcm.size == FRAME_SAMPLES) || (pcm == null && end))
        val index = codec.dequeueInputBuffer(0)
        if (index < 0) return false
        val size = if (pcm == null) 0 else {
            val buffer = requireNotNull(codec.getInputBuffer(index))
            buffer.clear()
            buffer.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(pcm)
            FRAME_SAMPLES * Short.SIZE_BYTES
        }
        codec.queueInputBuffer(index, 0, size, presentationTimeUs, if (end) MediaCodec.BUFFER_FLAG_END_OF_STREAM else 0)
        endSubmitted = end
        return true
    }

    /** Delivers at most one bounded audio packet; codec configuration is not an audio packet. */
    fun poll(onPacket: (ByteArray) -> Unit): Output {
        val index = codec.dequeueOutputBuffer(info, 0)
        if (index == MediaCodec.INFO_TRY_AGAIN_LATER) return Output.Empty
        if (index < 0) return Output.Progress
        try {
            if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                require(info.size <= MAX_PACKET_BYTES) { "encoded packet exceeds the lab limit" }
                val packet = ByteArray(info.size)
                requireNotNull(codec.getOutputBuffer(index)).apply { position(info.offset); get(packet) }
                onPacket(packet)
            }
            return if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) Output.End else Output.Progress
        } finally {
            codec.releaseOutputBuffer(index, false)
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
                return OpusEncoder(codec)
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
