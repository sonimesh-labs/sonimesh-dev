package com.sonimesh.opusspike

import android.media.MediaCodec
import android.media.MediaFormat
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import android.content.Context
import java.io.DataOutputStream
import java.io.File
import java.nio.ByteOrder
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import kotlin.math.PI
import kotlin.math.sin

@RunWith(AndroidJUnit4::class)
class OpusInteropTest {
    @Test
    fun encodeSyntheticToneForDesktop() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val file = File(requireNotNull(context.getExternalFilesDir(null)), "tone.opusframes")
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        try {
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
            format.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            DataOutputStream(file.outputStream().buffered()).use { output ->
                val info = MediaCodec.BufferInfo()
                var submitted = 0
                var packets = 0
                var finished = false
                val deadline = android.os.SystemClock.elapsedRealtime() + 30_000
                while (!finished && android.os.SystemClock.elapsedRealtime() < deadline) {
                    if (submitted <= 100) {
                        val index = codec.dequeueInputBuffer(10_000)
                        if (index >= 0) {
                            val input = requireNotNull(codec.getInputBuffer(index))
                            input.clear()
                            input.order(ByteOrder.LITTLE_ENDIAN)
                            if (submitted < 100) {
                                repeat(960) { sample ->
                                    val value = (sin(2.0 * PI * sample / 48.0) * 8_000).toInt().toShort()
                                    input.putShort(value)
                                    input.putShort(value)
                                }
                            }
                            val size = input.position()
                            codec.queueInputBuffer(index, 0, size, submitted * 20_000L,
                                if (submitted == 100) MediaCodec.BUFFER_FLAG_END_OF_STREAM else 0)
                            submitted++
                        }
                    }
                    val index = codec.dequeueOutputBuffer(info, 10_000)
                    if (index >= 0) {
                        val encoded = requireNotNull(codec.getOutputBuffer(index))
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                            require(info.size <= 1_148)
                            val packet = ByteArray(info.size)
                            encoded.position(info.offset)
                            encoded.get(packet)
                            output.writeInt(packet.size)
                            output.write(packet)
                            packets++
                        }
                        finished = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        codec.releaseOutputBuffer(index, false)
                    }
                }
                assertTrue("encoder did not finish", finished)
                assertTrue("encoder produced no packets", packets > 0)
            }
        } finally {
            codec.release()
        }
    }
}
