package com.sonimesh.sender.encoder

import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OpusEncoderTest {
    @Test
    fun encodesFramesAndDrainsEndOfStream() {
        OpusEncoder.create().use { encoder ->
            assertThrows(IllegalArgumentException::class.java) { encoder.tryQueue(ShortArray(1), 0) }
            val frame = ShortArray(OpusEncoder.FRAME_SAMPLES) { if (it % 96 < 48) 2_000 else -2_000 }
            val deadline = SystemClock.elapsedRealtime() + 5_000
            var queued = 0
            var endSubmitted = false
            var ended = false
            var packets = 0
            while (!ended && SystemClock.elapsedRealtime() < deadline) {
                if (queued < 10 && encoder.tryQueue(frame, queued * 20_000L)) queued++
                if (queued == 10 && !endSubmitted) {
                    endSubmitted = encoder.tryQueue(null, queued * 20_000L, end = true)
                }
                ended = encoder.poll { packet ->
                    assertTrue(packet.isNotEmpty() && packet.size <= OpusEncoder.MAX_PACKET_BYTES)
                    packets++
                } == OpusEncoder.Output.End
                if (!ended) Thread.sleep(2)
            }
            assertTrue("encoder did not accept all frames", queued == 10)
            assertTrue("encoder did not accept end of stream", endSubmitted)
            assertTrue("encoder did not finish", ended)
            assertTrue("encoder produced no audio packets", packets > 0)
            assertThrows(IllegalStateException::class.java) { encoder.tryQueue(frame, 220_000) }
        }
    }
}
