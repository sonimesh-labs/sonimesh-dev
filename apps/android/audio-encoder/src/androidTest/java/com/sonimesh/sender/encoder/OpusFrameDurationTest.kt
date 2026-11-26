package com.sonimesh.sender.encoder

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.SystemClock
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.nio.ByteOrder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Observes actual MediaCodec packet durations; input-buffer length alone does not determine them. */
@RunWith(AndroidJUnit4::class)
class OpusFrameDurationTest {
    @Test
    fun reportsPacketDurationsForTenAndTwentyMillisecondInputs() {
        assertEquals(10_000, packetDurationUs(byteArrayOf((18 shl 3).toByte())))
        assertEquals(20_000, packetDurationUs(byteArrayOf((19 shl 3).toByte())))
        assertEquals(40_000, packetDurationUs(byteArrayOf(((19 shl 3) or 1).toByte())))
        for (inputMs in listOf(10, 20)) observe(inputMs)
    }

    private fun observe(inputMs: Int) {
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        try {
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
            format.setInteger(MediaFormat.KEY_BIT_RATE, OpusBitrate.STANDARD.bitsPerSecond)
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            val inputSamples = inputMs * 96
            val frame = ShortArray(inputSamples) { if (it % 96 < 48) 2_000 else -2_000 }
            val info = MediaCodec.BufferInfo()
            val durations = mutableListOf<Int>()
            val deadline = SystemClock.elapsedRealtime() + 10_000
            var submitted = 0
            var endSubmitted = false
            var ended = false
            while (!ended && SystemClock.elapsedRealtime() < deadline) {
                if (!endSubmitted) {
                    val index = codec.dequeueInputBuffer(0)
                    if (index >= 0) {
                        if (submitted < 40) {
                            val buffer = requireNotNull(codec.getInputBuffer(index))
                            buffer.clear()
                            buffer.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(frame)
                            codec.queueInputBuffer(index, 0, inputSamples * 2, submitted * inputMs * 1_000L, 0)
                            submitted++
                        } else {
                            codec.queueInputBuffer(index, 0, 0, submitted * inputMs * 1_000L, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                            endSubmitted = true
                        }
                    }
                }
                val index = codec.dequeueOutputBuffer(info, 0)
                if (index >= 0) {
                    try {
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                            val packet = ByteArray(info.size)
                            requireNotNull(codec.getOutputBuffer(index)).apply { position(info.offset); get(packet) }
                            durations.add(packetDurationUs(packet))
                        }
                        ended = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                    } finally {
                        codec.releaseOutputBuffer(index, false)
                    }
                }
                if (!ended) Thread.sleep(2)
            }
            assertTrue("encoder did not drain", ended)
            assertTrue("encoder produced no audio packets", durations.isNotEmpty())
            Log.i("SoniMeshOpusDurations", "codec=${codec.name} input_ms=$inputMs packets=${durations.size} durations_us=${durations.groupingBy { it }.eachCount()}")
        } finally {
            codec.release()
        }
    }

    private fun packetDurationUs(packet: ByteArray): Int {
        require(packet.isNotEmpty())
        val toc = packet[0].toInt() and 0xff
        val config = toc ushr 3
        val frameUs = when {
            config >= 16 -> 2_500 shl (config and 3)
            config >= 12 -> 10_000 shl (config and 1)
            config and 3 == 3 -> 60_000
            else -> 10_000 shl (config and 3)
        }
        val count = when (toc and 3) {
            0 -> 1
            1, 2 -> 2
            else -> { require(packet.size > 1); packet[1].toInt() and 0x3f }
        }
        require(count > 0 && frameUs * count <= 120_000)
        return frameUs * count
    }
}
