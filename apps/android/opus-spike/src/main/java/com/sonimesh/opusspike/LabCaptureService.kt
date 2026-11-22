package com.sonimesh.opusspike

import android.annotation.SuppressLint
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.MediaCodec
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.ByteOrder
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class LabCaptureService : Service() {
    private val running = AtomicBoolean(false)
    private val socketLock = Any()
    private var socket: Socket? = null
    private var projection: MediaProjection? = null
    private val handler = Handler(Looper.getMainLooper())

    override fun onBind(intent: Intent?): IBinder? = null

    @Suppress("DEPRECATION")
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") {
            stopSelf()
            return START_NOT_STICKY
        }
        if (!running.compareAndSet(false, true)) return START_NOT_STICKY
        try {
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(NotificationChannel("lab", "Audio lab", NotificationManager.IMPORTANCE_LOW))
            val stop = PendingIntent.getService(this, 0, Intent(this, LabCaptureService::class.java).setAction("stop"),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            startForeground(1, Notification.Builder(this, "lab")
                .setSmallIcon(android.R.drawable.ic_btn_speak_now)
                .setContentTitle("SoniMesh audio lab capturing")
                .addAction(Notification.Action.Builder(null, "Stop", stop).build()).build())
            val result = intent?.getParcelableExtra<Intent>("resultData") ?: error("missing consent")
            val code = intent.getIntExtra("resultCode", Activity.RESULT_CANCELED)
            val current = getSystemService(MediaProjectionManager::class.java).getMediaProjection(code, result)
                ?: error("projection unavailable")
            projection = current
            current.registerCallback(object : MediaProjection.Callback() {
                override fun onStop() { stopSelf() }
            }, handler)
            Thread({ capture(current) }, "audio-lab-capture").start()
        } catch (error: Exception) {
            Log.e("SoniMeshLab", "Unable to start capture", error)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    @SuppressLint("MissingPermission")
    private fun capture(projection: MediaProjection) {
        var record: AudioRecord? = null
        var codec: MediaCodec? = null
        try {
            val connection = Socket()
            synchronized(socketLock) {
                if (!running.get()) { connection.close(); return }
                socket = connection
            }
            connection.connect(InetSocketAddress("127.0.0.1", 39091), 2_000)
            val packets = ArrayBlockingQueue<ByteArray>(8)
            Thread({ writePackets(connection, packets) }, "audio-lab-sender").start()
            val captureConfig = AudioPlaybackCaptureConfiguration.Builder(projection)
                .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                .addMatchingUsage(AudioAttributes.USAGE_GAME)
                .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN).build()
            val minimum = AudioRecord.getMinBufferSize(48_000, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_16BIT)
            require(minimum in 1..262_144)
            val activeRecord = AudioRecord.Builder().setAudioPlaybackCaptureConfig(captureConfig)
                .setAudioFormat(AudioFormat.Builder().setSampleRate(48_000)
                    .setChannelMask(AudioFormat.CHANNEL_IN_STEREO).setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(maxOf(minimum, 3_840)).build()
            record = activeRecord
            check(activeRecord.state == AudioRecord.STATE_INITIALIZED && activeRecord.sampleRate == 48_000 && activeRecord.channelCount == 2)
            val activeCodec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
            codec = activeCodec
            val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, 48_000, 2)
            format.setInteger(MediaFormat.KEY_BIT_RATE, 96_000)
            activeCodec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            activeCodec.start()
            activeRecord.startRecording()
            val pcm = ShortArray(1_920)
            val info = MediaCodec.BufferInfo()
            var filled = 0
            var frame = 0L
            while (running.get()) {
                if (filled < pcm.size) {
                    val read = activeRecord.read(pcm, filled, pcm.size - filled, AudioRecord.READ_NON_BLOCKING)
                    check(read >= 0 && read % 2 == 0)
                    filled += read
                    if (read == 0) Thread.sleep(2)
                }
                if (filled == pcm.size) {
                    val index = activeCodec.dequeueInputBuffer(0)
                    if (index >= 0) {
                        val input = requireNotNull(activeCodec.getInputBuffer(index))
                        input.clear()
                        input.order(ByteOrder.LITTLE_ENDIAN).asShortBuffer().put(pcm)
                        activeCodec.queueInputBuffer(index, 0, pcm.size * Short.SIZE_BYTES, frame * 20_000, 0)
                        frame++
                        filled = 0
                    }
                }
                val index = activeCodec.dequeueOutputBuffer(info, 0)
                if (index >= 0) {
                    if (info.size > 0 && info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        require(info.size <= 1_148)
                        val packet = ByteArray(info.size)
                        requireNotNull(activeCodec.getOutputBuffer(index)).apply { position(info.offset); get(packet) }
                        if (!packets.offer(packet)) {
                            packets.poll()
                            packets.offer(packet)
                        }
                    }
                    activeCodec.releaseOutputBuffer(index, false)
                }
            }
        } catch (error: Exception) {
            if (running.get()) Log.e("SoniMeshLab", "Capture or encoding failed", error)
        } finally {
            try { record?.stop() } catch (_: IllegalStateException) { }
            record?.release()
            codec?.release()
            handler.post { stopSelf() }
        }
    }

    private fun writePackets(connection: Socket, packets: ArrayBlockingQueue<ByteArray>) {
        try {
            val output = DataOutputStream(connection.getOutputStream())
            while (running.get()) {
                val packet = packets.poll(100, TimeUnit.MILLISECONDS) ?: continue
                output.writeInt(packet.size)
                output.write(packet)
            }
        } catch (error: Exception) {
            if (running.get()) Log.e("SoniMeshLab", "Desktop connection failed", error)
        } finally {
            handler.post { stopSelf() }
        }
    }

    override fun onDestroy() {
        running.set(false)
        synchronized(socketLock) {
            try { socket?.close() } catch (_: java.io.IOException) { }
            socket = null
        }
        projection?.stop()
        projection = null
        super.onDestroy()
    }
}
