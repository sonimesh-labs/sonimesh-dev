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
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import com.sonimesh.sender.encoder.OpusBitrate
import com.sonimesh.sender.encoder.OpusEncoder
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
import java.security.SecureRandom
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

internal fun incrementSaturated(value: Long): Long =
    if (value == Long.MAX_VALUE) value else value + 1

class LabCaptureService : Service() {
    private val running = AtomicBoolean(false)
    private val socketLock = Any()
    private var socket: Socket? = null
    private val setupSocket = SetupSocket()
    private var projection: MediaProjection? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private val handler = Handler(Looper.getMainLooper())
    private var terminalStatus = false
    private var started = false
    private var workerStarted = false
    private var workerFinished = false
    private var destroyed = false

    override fun onBind(intent: Intent?): IBinder? = null

    @Suppress("DEPRECATION")
    @SuppressLint("WakelockTimeout") // Released when this foreground service is destroyed.
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") {
            if (started) requestStop() else stopSelf()
            return START_NOT_STICKY
        }
        if (started) return START_NOT_STICKY
        started = true
        val usage = LabCaptureUsage.fromValue(intent?.getStringExtra("captureUsage"))
        val bitrate = OpusBitrate.fromBitsPerSecond(intent?.getIntExtra("bitrate", OpusBitrate.STANDARD.bitsPerSecond) ?: OpusBitrate.STANDARD.bitsPerSecond)
        val udpMode = intent?.getBooleanExtra("udpMode", false) ?: false
        val pairedMode = intent?.getBooleanExtra("pairedMode", false) ?: false
        val transport = LabTransport.fromFlags(udpMode, pairedMode)
        LabStatus.serviceStarted(usage, bitrate, transport)
        running.set(true)
        LabStatus.publishSenderQueueDrops(null)
        LabStatus.publishSentPackets(null)
        LabStatus.publishEncoderRate(null)
        LabStatus.publish("Connecting to the desktop listener…")
        var pendingInvitation: ByteArray? = null
        try {
            val manager = getSystemService(NotificationManager::class.java)
            manager.createNotificationChannel(NotificationChannel("lab", "Audio lab", NotificationManager.IMPORTANCE_LOW))
            val stop = PendingIntent.getService(this, 0, Intent(this, LabCaptureService::class.java).setAction("stop"),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            val open = PendingIntent.getActivity(this, 0, Intent(this, LabActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            }, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            startForeground(1, Notification.Builder(this, "lab")
                .setSmallIcon(android.R.drawable.ic_btn_speak_now)
                .setContentTitle("SoniMesh audio lab capturing")
                .setContentText("Transport: ${transport.label}")
                .setContentIntent(open)
                .addAction(Notification.Action.Builder(null, "Stop", stop).build()).build())
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "SoniMesh:LabCapture").also { it.acquire() }
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
            val destination = if (udpMode) parseLabAddress(intent.getStringExtra("udpAddress") ?: "")
                ?: error("invalid desktop address") else null
            pendingInvitation = if (pairedMode) intent.getByteArrayExtra("pairedInvitation")
                ?: labInvitation(intent.getStringExtra("pairedPin") ?: "",
                    intent.getStringExtra("pairedHost") ?: "127.0.0.1") else null
            val invitation = pendingInvitation
            Thread({ capture(current, udpMode, destination, invitation, usage, bitrate) }, "audio-lab-capture").start()
            workerStarted = true
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
        bitrate: OpusBitrate,
    ) {
        var record: AudioRecord? = null
        var codec: OpusEncoder? = null
        var paired: PairedConnection? = null
        var pairedIdentity: StreamIdentity? = null
        var connected = false
        var pairedControlFailed = false
        var stopResult: Result<StreamControlResult>? = null
        val pairedProducerFinished = AtomicBoolean(false)
        var pairedSender: Thread? = null
        var flushEncoderTail: (() -> Boolean)? = null
        var senderQueueDrops = 0L
        var lastDropPublication = 0L
        var captureStarted = false
        var rateMeter: EncoderRateMeter? = null
        var lastRatePublication = 0L
        val sentPackets = AtomicLong(0)
        val lastSentPublication = AtomicLong(0)
        val packetSent: () -> Unit = {
            val count = sentPackets.updateAndGet(::incrementSaturated)
            val now = SystemClock.elapsedRealtime()
            val previous = lastSentPublication.get()
            if (now - previous >= 1_000 && lastSentPublication.compareAndSet(previous, now)) {
                handler.post { LabStatus.publishSentPackets(count) }
            }
        }
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
                val parameters = StreamParameters(identity, 48_000, 2, 20)
                val result = startPairedWithRetry(running::get, Thread::sleep, onRetry = {
                    handler.post {
                        if (running.get() && !terminalStatus) LabStatus.publish("Desktop receiver is not ready. Retrying…")
                    }
                }) { connection.startStream(parameters) }
                requirePairedStart(result)
                pairedIdentity = identity
                check(setupSocket.handoff()) { "capture stopped while starting stream" }
                check(running.get()) { "capture stopped while starting stream" }
                val sealer = LabMediaSealer(connection.mediaRoot, session, 1, 0)
                pairedSender = Thread({ sendDatagrams(sealer, packets, requireNotNull(destination), pairedProducerFinished, packetSent) }, "audio-lab-sender")
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
                    Thread({ sendDatagrams(sealer, packets, requireNotNull(destination), onSent = packetSent) }, "audio-lab-sender").start()
                } else {
                    Thread({ writePackets(connection, packets, packetSent) }, "audio-lab-sender").start()
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
            val activeCodec = OpusEncoder.create(bitrate)
            codec = activeCodec
            activeRecord.startRecording()
            captureStarted = true
            val captureStartMillis = SystemClock.elapsedRealtime()
            val activeRateMeter = EncoderRateMeter(captureStartMillis)
            rateMeter = activeRateMeter
            lastRatePublication = captureStartMillis
            handler.post {
                if (running.get() && !terminalStatus) {
                    LabStatus.publishSenderQueueDrops(0)
                    LabStatus.publishSentPackets(0)
                    LabStatus.publish("Capturing audio and sending to desktop.")
                }
            }
            val pcm = ShortArray(OpusEncoder.FRAME_SAMPLES)
            val signalMonitor = LabSignalMonitor()
            val encoderInputWait = EncoderInputWait()
            val captureReadWait = CaptureReadWait()
            var filled = 0
            var frame = 0L
            var outputSequence = 0L
            fun pollEncodedOutput(): EncoderTailOutput {
                return when (activeCodec.poll { packet ->
                    check(outputSequence <= 0xffff_ffffL) { "media sequence exhausted" }
                    activeRateMeter.record(packet.size)
                    val now = SystemClock.elapsedRealtime()
                    if (now - lastRatePublication >= 1_000) {
                        lastRatePublication = now
                        val rate = activeRateMeter.averageKbps(now)
                        handler.post { LabStatus.publishEncoderRate(rate) }
                    }
                    val encoded = EncodedPacket(outputSequence++, packet)
                    if (enqueueLatest(packets, encoded)) {
                        if (senderQueueDrops < Long.MAX_VALUE) senderQueueDrops++
                        if (lastDropPublication == 0L || now - lastDropPublication >= 1_000) {
                            lastDropPublication = now
                            val count = senderQueueDrops
                            handler.post { LabStatus.publishSenderQueueDrops(count) }
                        }
                    }
                }) {
                    OpusEncoder.Output.Empty -> EncoderTailOutput.Empty
                    OpusEncoder.Output.Progress -> EncoderTailOutput.Progress
                    OpusEncoder.Output.End -> EncoderTailOutput.End
                }
            }
            flushEncoderTail = {
                drainEncoderTail(
                    nowMillis = SystemClock::elapsedRealtime,
                    submitEnd = {
                        if (filled > 0) pcm.fill(0, filled, pcm.size)
                        activeCodec.tryQueue(if (filled > 0) pcm else null, frame * 20_000, end = true)
                    },
                    pollOutput = ::pollEncodedOutput,
                    idle = { Thread.sleep(2) },
                )
            }
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
                        handler.post { finishWith("Desktop connection ended. Check the listener, then start capture again.") }
                        break
                    }
                    nextControlPoll = SystemClock.elapsedRealtime() + 250
                }
                if (filled < pcm.size) {
                    val read = activeRecord.read(pcm, filled, pcm.size - filled, AudioRecord.READ_NON_BLOCKING)
                    check(read >= 0 && read <= pcm.size - filled && read % 2 == 0)
                    captureReadWait.observe(read, SystemClock.elapsedRealtime())
                    if (read > 0) {
                        val transition = signalMonitor.observe(pcm, filled, read)
                        if (transition != null) handler.post {
                            if (running.get() && !terminalStatus) {
                                LabStatus.publish(when (transition) {
                                    LabSignalTransition.NoCapturableAudio -> {
                                        LabStatus.publishSignal(LabSignalState.Silent)
                                        "No capturable audio detected. The source may be silent or capture-limited."
                                    }
                                    LabSignalTransition.SignalDetected -> {
                                        LabStatus.publishSignal(LabSignalState.Detected)
                                        "Capturing audio and sending to desktop."
                                    }
                                })
                            }
                        }
                    }
                    filled += read
                    if (read == 0) Thread.sleep(2)
                }
                var waitingForInput = false
                if (filled == pcm.size) {
                    if (activeCodec.tryQueue(pcm, frame * 20_000)) {
                        frame++
                        filled = 0
                        encoderInputWait.reset()
                    } else {
                        waitingForInput = true
                    }
                }
                val output = pollEncodedOutput()
                if (waitingForInput && encoderInputWait.shouldIdle(SystemClock.elapsedRealtime(), output)) {
                    Thread.sleep(2)
                }
            }
        } catch (error: Exception) {
            if (running.get()) {
                Log.e("SoniMeshLab", "Capture or encoding failed", error)
                val message = if (error is EncoderStalledException) "Audio encoder stopped responding. Restart capture and try again."
                    else if (error is CaptureReadStalledException) "Audio capture stopped producing samples. Restart capture and try again."
                    else if (error is PairedStartRejectedException && error.retryable) "Desktop receiver is not ready. Check its output device and try again."
                    else if (error is PairedStartRejectedException) "Desktop rejected the audio stream. Check the listener settings and try again."
                    else if (error is PairedStartStatusException) "Desktop did not start the audio stream. Check the listener and try again."
                    else if (connected) "Capture or encoding failed. Check device support and try again."
                    else "Desktop connection failed. Check the listener and control address."
                handler.post { if (running.get() && !terminalStatus) finishWith(message) }
            }
        } finally {
            try { record?.stop() } catch (_: IllegalStateException) { }
            record?.release()
            if (!running.get() && paired != null && !pairedControlFailed && pairedSender?.isAlive == true) {
                try {
                    if (flushEncoderTail?.invoke() == false) Log.w("SoniMeshLab", "Encoder tail drain timed out")
                } catch (error: Exception) {
                    if (error is InterruptedException) Thread.currentThread().interrupt()
                    Log.w("SoniMeshLab", "Encoder tail drain failed", error)
                }
            }
            if (captureStarted) {
                val count = senderQueueDrops
                val rate = rateMeter?.averageKbps(SystemClock.elapsedRealtime())
                handler.post {
                    LabStatus.publishSenderQueueDrops(count)
                    LabStatus.publishEncoderRate(rate)
                }
            }
            codec?.close()
            if (paired != null) {
                finishPairedSender(pairedProducerFinished, pairedSender)
                try {
                    stopResult = attemptPairedStop(pairedIdentity, pairedControlFailed, paired::stopStream)
                    stopResult?.exceptionOrNull()?.let { Log.e("SoniMeshLab", "Paired stream stop failed", it) }
                } finally {
                    paired.close()
                }
            }
            if (captureStarted) {
                handler.post { LabStatus.publishSentPackets(sentPackets.get()) }
            }
            handler.post {
                workerFinished = true
                captureStopWarning(terminalStatus, stopResult)?.let {
                    terminalStatus = true
                    LabStatus.publish(it)
                }
                if (destroyed) {
                    LabStatus.serviceFinished()
                    if (!terminalStatus) LabStatus.publish("Capture stopped.")
                } else stopSelf()
            }
        }
    }

    private data class EncodedPacket(val sequence: Long, val bytes: ByteArray)

    private fun writePackets(connection: Socket, packets: ArrayBlockingQueue<EncodedPacket>, onSent: () -> Unit) {
        try {
            val output = DataOutputStream(connection.getOutputStream())
            while (running.get()) {
                val packet = packets.poll(100, TimeUnit.MILLISECONDS) ?: continue
                output.writeInt(packet.bytes.size)
                output.write(packet.bytes)
                onSent()
            }
        } catch (error: Exception) {
            if (running.get()) {
                Log.e("SoniMeshLab", "Desktop connection failed", error)
                handler.post { finishWith("Desktop connection ended. Restart the listener to reconnect.") }
            }
        }
    }

    private fun sendDatagrams(
        sealer: LabMediaSealer,
        packets: ArrayBlockingQueue<EncodedPacket>,
        destination: java.net.Inet4Address,
        producerFinished: AtomicBoolean? = null,
        onSent: () -> Unit = {},
    ) {
        try {
            DatagramSocket().use { udp ->
                udp.connect(destination, 39093)
                val send = { packet: EncodedPacket ->
                    val datagram = sealer.seal(packet.sequence, packet.sequence * 960, 0, packet.bytes)
                    udp.send(DatagramPacket(datagram, datagram.size))
                    onSent()
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
                handler.post { finishWith("UDP sending failed. Check the network and listener, then start capture again.") }
            }
        } finally {
            sealer.clear()
        }
    }

    private fun watchControl(connection: Socket) {
        try {
            check(connection.getInputStream().read() == -1) { "unexpected lab control data" }
        } catch (error: Exception) {
            if (running.get()) Log.e("SoniMeshLab", "Lab control connection ended", error)
        } finally {
            if (running.get()) handler.post { finishWith("Desktop connection ended. Restart the listener to reconnect.") }
        }
    }

    private fun finishWith(message: String) {
        if (!running.get() || terminalStatus) return
        terminalStatus = true
        LabStatus.publish(message)
        requestStop()
    }

    private fun requestStop() {
        if (!running.getAndSet(false)) return
        if (!terminalStatus) LabStatus.publish("Stopping capture…")
        setupSocket.cancel()
        synchronized(socketLock) {
            try { socket?.close() } catch (_: java.io.IOException) { }
            socket = null
        }
        projection?.stop()
        projection = null
        if (!workerStarted) stopSelf()
    }

    override fun onDestroy() {
        try {
            requestStop()
        } finally {
            wakeLock?.let { if (it.isHeld) it.release() }
            wakeLock = null
        }
        if (!terminalStatus) {
            LabStatus.publish(if (workerStarted && !workerFinished) "Stopping capture…" else "Capture stopped.")
        }
        destroyed = true
        if (started && (!workerStarted || workerFinished)) LabStatus.serviceFinished()
        super.onDestroy()
    }
}
