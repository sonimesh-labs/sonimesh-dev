package com.sonimesh.sender

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import com.sonimesh.sender.capture.CaptureSource
import kotlin.math.PI
import kotlin.math.sin

internal fun captureTestToneSamples(sampleRate: Int): ShortArray =
    ShortArray(sampleRate / 10) { frame ->
        (sin(2.0 * PI * 440 * frame / sampleRate) * 3_000).toInt().toShort()
    }

internal class CaptureTestTone : AutoCloseable {
    private var track: AudioTrack? = null

    fun start(source: CaptureSource): Boolean {
        close()
        for (sampleRate in listOf(48_000, 44_100)) {
            val samples = captureTestToneSamples(sampleRate)
            var candidate: AudioTrack? = null
            try {
                candidate = AudioTrack.Builder()
                    .setAudioAttributes(
                        AudioAttributes.Builder().setUsage(source.usages.first())
                            .setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_ALL).build(),
                    )
                    .setAudioFormat(
                        AudioFormat.Builder().setSampleRate(sampleRate)
                            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                            .setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build(),
                    )
                    .setTransferMode(AudioTrack.MODE_STATIC)
                    .setBufferSizeInBytes(samples.size * Short.SIZE_BYTES)
                    .build()
                check(candidate.write(samples, 0, samples.size) == samples.size)
                check(candidate.state == AudioTrack.STATE_INITIALIZED)
                check(candidate.setLoopPoints(0, samples.size, -1) == AudioTrack.SUCCESS)
                candidate.play()
                check(candidate.playState == AudioTrack.PLAYSTATE_PLAYING)
                track = candidate
                return true
            } catch (_: RuntimeException) {
                runCatching { candidate?.release() }
            }
        }
        return false
    }

    override fun close() {
        val previous = track ?: return
        track = null
        try {
            previous.stop()
        } catch (_: IllegalStateException) {
            // Release the track even if the device already stopped it.
        } finally {
            previous.release()
        }
    }
}
