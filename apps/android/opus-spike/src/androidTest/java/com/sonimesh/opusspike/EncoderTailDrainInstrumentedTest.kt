package com.sonimesh.opusspike

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.nio.ByteOrder
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class EncoderTailDrainInstrumentedTest {
    @Test
    fun finishesPendingOpusOutputAfterEndOfStream() {
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        try {
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
            format.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            val first = codec.dequeueInputBuffer(2_000_000)
            assertTrue("encoder did not accept initial input", first >= 0)
            val input = requireNotNull(codec.getInputBuffer(first))
            input.clear()
            input.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(ShortArray(1_920) { 2_000 })
            codec.queueInputBuffer(first, 0, 3_840, 0, 0)

            val info = MediaCodec.BufferInfo()
            var packets = 0
            Thread.currentThread().interrupt()
            val finished = drainEncoderTail(
                nowMillis = SystemClock::elapsedRealtime,
                submitEnd = {
                    val index = codec.dequeueInputBuffer(0)
                    if (index < 0) false else {
                        val tail = requireNotNull(codec.getInputBuffer(index))
                        tail.clear()
                        tail.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer()
                            .put(ShortArray(1_920) { if (it < 400) 2_000 else 0 })
                        codec.queueInputBuffer(index, 0, 3_840, 20_000, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        true
                    }
                },
                pollOutput = {
                    val index = codec.dequeueOutputBuffer(info, 0)
                    if (index < 0) EncoderTailOutput.Empty else {
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) packets++
                        val end = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        codec.releaseOutputBuffer(index, false)
                        if (end) EncoderTailOutput.End else EncoderTailOutput.Progress
                    }
                },
                idle = { Thread.sleep(2) },
            )
            assertTrue("encoder did not report end of stream within the lab deadline", finished)
            assertTrue("encoder produced no audio packets", packets > 0)
            assertTrue("stop interrupt should be preserved", Thread.currentThread().isInterrupted)
        } finally {
            Thread.interrupted()
            codec.release()
        }
    }
}
