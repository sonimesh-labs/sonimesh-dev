package com.sonimesh.opusspike

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.SystemClock
import androidx.test.platform.app.InstrumentationRegistry
import com.sonimesh.sender.pairing.PairingClient
import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import com.sonimesh.sender.pairing.StreamParameters
import com.sonimesh.sender.protocol.PairingQr
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteOrder
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.sin

class PairedAudioInteropTest {
    @Test
    fun pairs_starts_sends_opus_and_stops() {
        val args = InstrumentationRegistry.getArguments()
        assumeTrue("paired audio lab requires pairingSpkiHex", args.containsKey("pairingSpkiHex"))
        val pin = requireNotNull(args.getString("pairingSpkiHex")).chunked(2).map { it.toInt(16).toByte() }.toByteArray()
        val invitation = ByteArray(PairingQr.MAX_PAYLOAD_BYTES)
        val size = PairingQr.encode(1, ByteArray(16) { 0x71 }, ByteArray(32) { 0x42 },
            args.getString("pairingHost") ?: "127.0.0.1", 47610, pin, 120_100, invitation)
        val identity = StreamIdentity(0x12345678, 1, 0)
        PairingClient.connect(invitation.copyOf(size)).use { connection ->
            connection.negotiateHello("android audio lab", listOf(2))
            val start = connection.startStream(StreamParameters(identity, 48_000, 2, 20))
            assertTrue(start is StreamControlResult.Status)
            assertEquals(2, (start as StreamControlResult.Status).state)
            val sealer = LabMediaSealer(connection.mediaRoot, identity.sessionId, identity.streamId.toInt(), 0)
            try {
                sendTone(sealer, args.getString("mediaHost") ?: "10.0.2.2")
            } finally {
                sealer.clear()
            }
            val stop = connection.stopStream(identity)
            assertTrue(stop is StreamControlResult.Status)
            assertEquals(4, (stop as StreamControlResult.Status).state)
        }
    }

    private fun sendTone(sealer: LabMediaSealer, mediaHost: String) {
        val codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        try {
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
            format.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            codec.start()
            DatagramSocket().use { socket ->
                socket.connect(InetAddress.getByName(mediaHost), 39093)
                val info = MediaCodec.BufferInfo()
                var submitted = 0
                var sent = 0L
                var finished = false
                val deadline = SystemClock.elapsedRealtime() + 20_000
                var nextSend: Long? = null
                while (!finished && SystemClock.elapsedRealtime() < deadline) {
                    if (submitted <= 100) {
                        val index = codec.dequeueInputBuffer(10_000)
                        if (index >= 0) {
                            val input = requireNotNull(codec.getInputBuffer(index))
                            input.clear()
                            input.order(ByteOrder.LITTLE_ENDIAN)
                            if (submitted < 100) repeat(960) { sample ->
                                val value = (sin(2.0 * PI * sample / 48.0) * 8_000).toInt().toShort()
                                input.putShort(value); input.putShort(value)
                            }
                            codec.queueInputBuffer(index, 0, input.position(), submitted * 20_000L,
                                if (submitted == 100) MediaCodec.BUFFER_FLAG_END_OF_STREAM else 0)
                            submitted++
                        }
                    }
                    val index = codec.dequeueOutputBuffer(info, 10_000)
                    if (index >= 0) {
                        if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                            val encoded = ByteArray(info.size)
                            requireNotNull(codec.getOutputBuffer(index)).apply { position(info.offset); get(encoded) }
                            val due = nextSend ?: SystemClock.elapsedRealtime()
                            val delay = due - SystemClock.elapsedRealtime()
                            if (delay > 0) Thread.sleep(delay)
                            val packet = sealer.seal(sent, sent * 960, 0, encoded)
                            socket.send(DatagramPacket(packet, packet.size))
                            sent++
                            nextSend = nextSendDeadline(due, SystemClock.elapsedRealtime())
                        }
                        finished = info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0
                        codec.releaseOutputBuffer(index, false)
                    }
                }
                assertTrue("encoder did not finish", finished)
                assertTrue("encoder produced too few packets", sent >= 90)
                Thread.sleep(250)
            }
        } finally {
            codec.release()
        }
    }

    @Test
    fun sender_pacing_preserves_cadence_and_rebases_after_a_stall() {
        assertEquals(1_020L, nextSendDeadline(1_000, 1_002))
        assertEquals(1_040L, nextSendDeadline(1_000, 1_020))
        assertEquals(1_520L, nextSendDeadline(1_000, 1_500))
    }
}

private fun nextSendDeadline(due: Long, now: Long): Long =
    (if (now - due >= 20) now else due) + 20
