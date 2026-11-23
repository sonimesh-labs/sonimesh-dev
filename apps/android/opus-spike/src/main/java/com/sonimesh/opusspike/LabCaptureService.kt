package com.sonimesh.opusspike

import android.annotation.SuppressLint
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
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
import android.os.SystemClock
import android.util.Log
import com.sonimesh.sender.pairing.PairedConnection
import com.sonimesh.sender.pairing.PairingClient
import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import com.sonimesh.sender.pairing.StreamParameters
import java.io.DataOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.ByteOrder
import java.security.SecureRandom
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

class LabCaptureService : Service() {
    private val running = AtomicBoolean(false)
    private val socketLock = Any()
    private var socket: Socket? = null
    private val setupSocket = SetupSocket()
    private var projection: MediaProjection? = null
    private val handler = Handler(Looper.getMainLooper())
    private var terminalStatus = false

    override fun onBind(intent: Intent?): IBinder? = null

    @Suppress("DEPRECATION")
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") {
            stopSelf()
            return START_NOT_STICKY
        }
        if (!running.compareAndSet(false, true)) return START_NOT_STICKY
        LabStatus.publish("Connecting to the desktop listener…")
        var pendingInvitation: ByteArray? = null
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
                override fun onStop() { finishWith("Screen capture ended. Grant consent again to restart.") }
            }, handler)
            val udpMode = intent.getBooleanExtra("udpMode", false)
            val pairedMode = intent.getBooleanExtra("pairedMode", false)
            val usage = LabCaptureUsage.fromValue(intent.getStringExtra("captureUsage"))
            val destination = if (udpMode) parseLabAddress(intent.getStringExtra("udpAddress") ?: "")
                ?: error("invalid desktop address") else null
            pendingInvitation = if (pairedMode) intent.getByteArrayExtra("pairedInvitation")
                ?: labInvitation(intent.getStringExtra("pairedPin") ?: "",
                    intent.getStringExtra("pairedHost") ?: "127.0.0.1") else null
            val invitation = pendingInvitation
            Thread({ capture(current, udpMode, destination, invitation, usage) }, "audio-lab-capture").start()
            pendingInvitation = null
        } catch (error: Exception) {
            pendingInvitation?.fill(0)
            Log.e("SoniMeshLab", "Unable to start capture", error)
            finishWith("Unable to start capture. Check permissions and try again.")
        }
        return START_NOT_STICKY
    }

    @SuppressLint("MissingPermission")
    private fun capture(
        projection: MediaProjection,
        udpMode: Boolean,
        destination: java.net.Inet4Address?,
        invitation: ByteArray?,
        usage: LabCaptureUsage,
    ) {
        var record: AudioRecord? = null
        var codec: MediaCodec? = null
        var paired: PairedConnection? = null
        var pairedIdentity: StreamIdentity? = null
        var connected = false
        var pairedControlFailed = false
        val pairedProducerFinished = AtomicBoolean(false)
        var pairedSender: Thread? = null
        try {
            val packets = ArrayBlockingQueue<EncodedPacket>(8)
            if (invitation != null) {
                val raw = Socket()
                check(setupSocket.track(raw)) { "capture stopped before pairing" }
                val connection = try {
                    PairingClient.connect(invitation, raw)
                } finally {
                    invitation.fill(0)
                }
                paired = connection
                check(setupSocket.track(connection.socket)) { "capture stopped during pairing" }
                check(running.get()) { "capture stopped during pairing" }
                connection.negotiateHello("android audio lab", listOf(2))
                val random = SecureRandom()
                var session = random.nextLong()
                while (session == 0L) session = random.nextLong()
                val identity = StreamIdentity(session, 1, 0)
                val result = connection.startStream(StreamParameters(identity, 48_000, 2, 20))
                check(result is StreamControlResult.Status && result.state == 2) { "desktop receiver not ready" }
                pairedIdentity = identity
                check(setupSocket.handoff()) { "capture stopped while starting stream" }
                check(running.get()) { "capture stopped while starting stream" }
                val sealer = LabMediaSealer(connection.mediaRoot, session, 1, 0)
                pairedSender = Thread({ sendDatagrams(sealer, packets, requireNotNull(destination), pairedProducerFinished) }, "audio-lab-sender")
                    .also { it.start() }
            } else {
                val connection = Socket()
                synchronized(socketLock) {
                    if (!running.get()) { connection.close(); return }
                    socket = connection
                }
                connection.connect(InetSocketAddress("127.0.0.1", if (udpMode) 39092 else 39091), 2_000)
                if (udpMode) {
                    val secret = ByteArray(32)
                    SecureRandom().nextBytes(secret)
                    val sealer = try {
                        connection.soTimeout = 5_000
                        connection.getOutputStream().write(secret)
                        check(connection.getInputStream().read() == 1) { "desktop receiver not ready" }
                        LabMediaSealer(secret, 1, 1, 0)
                    } finally {
                        secret.fill(0)
                    }
                    connection.soTimeout = 0
                    Thread({ watchControl(connection) }, "audio-lab-control").start()
                    Thread({ sendDatagrams(sealer, packets, requireNotNull(destination)) }, "audio-lab-sender").start()
                } else {
                    Thread({ writePackets(connection, packets) }, "audio-lab-sender").start()
                }
            }
            connected = true
            val captureConfigBuilder = AudioPlaybackCaptureConfiguration.Builder(projection)
            usage.usages.forEach(captureConfigBuilder::addMatchingUsage)
            val captureConfig = captureConfigBuilder.build()
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
            handler.post {
                if (running.get() && !terminalStatus) LabStatus.publish("Capturing audio and sending to desktop.")
            }
            val pcm = ShortArray(1_920)
            val signalMonitor = LabSignalMonitor()
            val info = MediaCodec.BufferInfo()
            var filled = 0
            var frame = 0L
            var outputSequence = 0L
            paired?.socket?.soTimeout = 1
            var nextControlPoll = SystemClock.elapsedRealtime()
            while (running.get()) {
                if (paired != null && SystemClock.elapsedRealtime() >= nextControlPoll) {
                    try {
                        val value = paired.socket.inputStream.read()
                        error(if (value == -1) "desktop closed control connection" else "unexpected control data")
                    } catch (_: SocketTimeoutException) {
                        // The control channel has no messages while capture is running.
                    } catch (error: Exception) {
                        pairedControlFailed = true
                        Log.e("SoniMeshLab", "Paired control connection ended", error)
                        handler.post { finishWith("Desktop connection ended. Restart the listener to reconnect.") }
                        break
                    }
                    nextControlPoll = SystemClock.elapsedRealtime() + 250
                }
                if (filled < pcm.size) {
                    val read = activeRecord.read(pcm, filled, pcm.size - filled, AudioRecord.READ_NON_BLOCKING)
                    check(read >= 0 && read <= pcm.size - filled && read % 2 == 0)
                    if (read > 0) {
                        val transition = signalMonitor.observe(pcm, filled, read)
                        if (transition != null) handler.post {
                            if (running.get() && !terminalStatus) {
                                LabStatus.publish(when (transition) {
                                    LabSignalTransition.NoCapturableAudio ->
                                        "No capturable audio detected. The source may be silent or capture-limited."
                                    LabSignalTransition.SignalDetected -> "Capturing audio and sending to desktop."
                                })
                            }
                        }
                    }
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
                        check(outputSequence <= 0xffff_ffffL) { "media sequence exhausted" }
                        val encoded = EncodedPacket(outputSequence++, packet)
                        if (!packets.offer(encoded)) {
                            packets.poll()
                            packets.offer(encoded)
                        }
                    }
                    activeCodec.releaseOutputBuffer(index, false)
                }
            }
        } catch (error: Exception) {
            if (running.get()) {
                Log.e("SoniMeshLab", "Capture or encoding failed", error)
                val message = if (connected) "Capture or encoding failed. Check device support and try again."
                    else "Desktop connection failed. Check the listener and control address."
                handler.post { finishWith(message) }
            }
        } finally {
            try { record?.stop() } catch (_: IllegalStateException) { }
            record?.release()
            codec?.release()
            if (paired != null) {
                finishPairedSender(pairedProducerFinished, pairedSender)
                try {
                    pairedIdentity?.takeUnless { pairedControlFailed }?.let { identity ->
                        val stopped = paired.stopStream(identity)
                        check(stopped is StreamControlResult.Status && stopped.state == 4) { "desktop did not stop stream" }
                    }
                } catch (error: Exception) {
                    Log.e("SoniMeshLab", "Paired stream stop failed", error)
                } finally {
                    paired.close()
                }
            }
            handler.post { stopSelf() }
        }
    }

    private data class EncodedPacket(val sequence: Long, val bytes: ByteArray)

    private fun writePackets(connection: Socket, packets: ArrayBlockingQueue<EncodedPacket>) {
        try {
            val output = DataOutputStream(connection.getOutputStream())
            while (running.get()) {
                val packet = packets.poll(100, TimeUnit.MILLISECONDS) ?: continue
                output.writeInt(packet.bytes.size)
                output.write(packet.bytes)
            }
        } catch (error: Exception) {
            if (running.get()) {
                Log.e("SoniMeshLab", "Desktop connection failed", error)
                handler.post { finishWith("Desktop connection ended. Restart the listener to reconnect.") }
            }
        } finally {
            handler.post { stopSelf() }
        }
    }

    private fun sendDatagrams(
        sealer: LabMediaSealer,
        packets: ArrayBlockingQueue<EncodedPacket>,
        destination: java.net.Inet4Address,
        producerFinished: AtomicBoolean? = null,
    ) {
        try {
            DatagramSocket().use { udp ->
                udp.connect(destination, 39093)
                val send = { packet: EncodedPacket ->
                    val datagram = sealer.seal(packet.sequence, packet.sequence * 960, 0, packet.bytes)
                    udp.send(DatagramPacket(datagram, datagram.size))
                }
                if (producerFinished != null) {
                    sendUntilProducerFinished(packets, producerFinished, send)
                } else {
                    while (running.get()) {
                        val packet = packets.poll(100, TimeUnit.MILLISECONDS) ?: continue
                        send(packet)
                    }
                }
            }
        } catch (error: Exception) {
            if (running.get()) {
                Log.e("SoniMeshLab", "UDP sender failed", error)
                handler.post { finishWith("UDP sending failed. Restart the lab session.") }
            }
        } finally {
            sealer.clear()
            handler.post { stopSelf() }
        }
    }

    private fun watchControl(connection: Socket) {
        try {
            check(connection.getInputStream().read() == -1) { "unexpected lab control data" }
        } catch (error: Exception) {
            if (running.get()) Log.e("SoniMeshLab", "Lab control connection ended", error)
        } finally {
            if (running.get()) handler.post { finishWith("Desktop connection ended. Restart the listener to reconnect.") }
            handler.post { stopSelf() }
        }
    }

    private fun finishWith(message: String) {
        if (!running.get() || terminalStatus) return
        terminalStatus = true
        LabStatus.publish(message)
        stopSelf()
    }

    override fun onDestroy() {
        running.set(false)
        setupSocket.cancel()
        if (!terminalStatus) LabStatus.publish("Capture stopped.")
        synchronized(socketLock) {
            try { socket?.close() } catch (_: java.io.IOException) { }
            socket = null
        }
        projection?.stop()
        projection = null
        super.onDestroy()
    }
}
