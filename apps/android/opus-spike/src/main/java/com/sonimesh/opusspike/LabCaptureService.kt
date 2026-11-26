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
import com.sonimesh.sender.pairing.StreamControlResult
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

internal fun encoderInput(pcm: ShortArray, silence: ShortArray, muted: Boolean): ShortArray =
    if (muted) silence else pcm

internal fun foregroundNotificationText(transport: String, connection: String?, muted: Boolean, signal: String): Pair<String, String> {
    val compact = buildList {
        add("Transport: $transport")
        connection?.let { add(it) }
        if (muted) add("Sending muted")
    }.joinToString(" · ")
    return compact to "$compact\n$signal"
}

private enum class ForegroundConnectionStatus(val label: String) {
    Connecting("Connecting to desktop"),
    Reconnecting("Reconnecting to desktop"),
    Restored("Desktop reconnected"),
}

class LabCaptureService : Service() {
    private val running = AtomicBoolean(false)
    private val senderMuted = AtomicBoolean(false)
    private val socketLock = Any()
    private var socket: Socket? = null
    private val setupSocket = SetupSocket()
    @Volatile private var activePairedSupervisor: PairedSessionSupervisor? = null
    @Volatile private var captureWorker: Thread? = null
    private var projection: MediaProjection? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var foregroundNotificationManager: NotificationManager? = null
    private var foregroundNotificationBuilder: Notification.Builder? = null
    private var stopAction: Notification.Action? = null
    private var muteAction: Notification.Action? = null
    private var unmuteAction: Notification.Action? = null
    private var foregroundTransportLabel: String? = null
    private var foregroundConnectionStatus: ForegroundConnectionStatus? = null
    private var foregroundSignalLabel = "Waiting for capture signal"
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
        if (intent?.action == ACTION_SET_MUTED) {
            if (started && running.get()) {
                val muted = intent.getBooleanExtra(EXTRA_MUTED, false)
                senderMuted.set(muted)
                LabStatus.publishSenderMuted(muted)
                updateForegroundActions(muted)
            } else if (!started) {
                stopSelf()
            }
            return START_NOT_STICKY
        }
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
            foregroundTransportLabel = transport.label
        val destinationPort = intent?.getIntExtra("udpPort", DEFAULT_LAB_MEDIA_PORT) ?: DEFAULT_LAB_MEDIA_PORT
        LabStatus.serviceStarted(usage, bitrate, transport, destinationPort.takeIf { udpMode && it in 1..65535 })
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
            fun muteIntent(muted: Boolean) = PendingIntent.getService(
                this, if (muted) 1 else 2,
                Intent(this, LabCaptureService::class.java).setAction(ACTION_SET_MUTED).putExtra(EXTRA_MUTED, muted),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            stopAction = Notification.Action.Builder(null, "Stop", stop).build()
            muteAction = Notification.Action.Builder(null, "Mute sending", muteIntent(true)).build()
            unmuteAction = Notification.Action.Builder(null, "Unmute sending", muteIntent(false)).build()
            val open = PendingIntent.getActivity(this, 0, Intent(this, LabActivity::class.java).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            }, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            val notification = Notification.Builder(this, "lab")
                .setSmallIcon(android.R.drawable.ic_btn_speak_now)
                .setContentTitle("SoniMesh audio lab capturing")
                .setContentText("Transport: ${transport.label}")
                .setSubText("Capture duration: —")
                .setContentIntent(open)
                .setActions(stopAction, muteAction)
            foregroundNotificationManager = manager
            foregroundNotificationBuilder = notification
            updateForegroundContentText(notification)
            startForeground(1, notification.build())
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
            require(!udpMode || destinationPort in 1..65535) { "invalid desktop UDP port" }
            pendingInvitation = if (pairedMode) intent.getByteArrayExtra("pairedInvitation")
                ?: labInvitation(intent.getStringExtra("pairedPin") ?: "",
                    intent.getStringExtra("pairedHost") ?: "127.0.0.1",
                    intent.getIntExtra("pairedControlPort", DEFAULT_LAB_CONTROL_PORT)) else null
            val invitation = pendingInvitation
            Thread({ capture(current, udpMode, destination, destinationPort, invitation, usage, bitrate) }, "audio-lab-capture")
                .also { captureWorker = it; it.start() }
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
        destinationPort: Int,
        invitation: ByteArray?,
        usage: LabCaptureUsage,
        bitrate: OpusBitrate,
    ) {
        var record: AudioRecord? = null
        var codec: OpusEncoder? = null
        var pairedSupervisor: PairedSessionSupervisor? = null
        var connected = false
        var stopResult: Result<StreamControlResult>? = null
        var tailFailed = false
        val pairedProducerFinished = AtomicBoolean(false)
        var flushEncoderTail: (() -> Boolean)? = null
        var senderQueueDrops = 0L
        var lastDropPublication = 0L
        var captureStarted = false
        var captureStartMillis = 0L
        var lastDurationPublication = 0L
        var rateMeter: EncoderRateMeter? = null
        var lastRatePublication = 0L
        val sentPackets = AtomicLong(0)
        val lastSentPublication = AtomicLong(0)
        val senderTiming = SenderTiming()
        val packetSent: () -> Unit = {
            val count = sentPackets.updateAndGet(::incrementSaturated)
            val timing = senderTiming.record(SystemClock.elapsedRealtimeNanos())
            val now = SystemClock.elapsedRealtime()
            val previous = lastSentPublication.get()
            if (now - previous >= 1_000 && lastSentPublication.compareAndSet(previous, now)) {
                handler.post {
                    LabStatus.publishSentPackets(count)
                    LabStatus.publishSenderTiming(timing)
                }
            }
        }
        try {
            val packets = ArrayBlockingQueue<EncodedPacket>(8)
            if (invitation != null) {
                val supervisor = PairedSessionSupervisor(
                    running = running,
                    producerFinished = pairedProducerFinished,
                    packets = packets,
                    invitation = invitation,
                    destination = requireNotNull(destination),
                    destinationPort = destinationPort,
                    setupSocket = setupSocket,
                    onStatus = { message ->
                        handler.post {
                            if (running.get() && !terminalStatus) {
                                LabStatus.publish(message)
                                updateForegroundConnectionStatus(message)
                            }
                        }
                    },
                    onInitialSessionStarted = {
                        handler.post {
                            if (running.get() && !terminalStatus) updateForegroundConnectionStatus(null)
                        }
                    },
                    onReconnectFailure = { message ->
                        handler.post { finishWith(message) }
                    },
                    onPacketSent = packetSent,
                )
                pairedSupervisor = supervisor
                activePairedSupervisor = supervisor
                supervisor.start()
                supervisor.awaitStarted()
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
                    Thread({ sendDatagrams(sealer, packets, requireNotNull(destination), destinationPort, onSent = packetSent) }, "audio-lab-sender").start()
                } else {
                    Thread({ writePackets(connection, packets, packetSent) }, "audio-lab-sender").start()
                }
            }
            connected = true
            handler.post {
                if (running.get() && !terminalStatus) {
                    publishCaptureStatus("Preparing audio capture and encoder…")
                }
            }
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
            captureStartMillis = SystemClock.elapsedRealtime()
            lastDurationPublication = captureStartMillis
            handler.post { LabStatus.publishCaptureDuration(0) }
            handler.post { updateForegroundDuration(0) }
            val activeRateMeter = EncoderRateMeter(captureStartMillis)
            rateMeter = activeRateMeter
            lastRatePublication = captureStartMillis
            handler.post {
                if (running.get() && !terminalStatus) {
                    LabStatus.publishSenderQueueDrops(0)
                    LabStatus.publishSentPackets(0)
                    publishCaptureStatus("Capturing audio and sending to desktop.")
                }
            }
            val pcm = ShortArray(OpusEncoder.FRAME_SAMPLES)
            val silence = ShortArray(OpusEncoder.FRAME_SAMPLES)
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
                        activeCodec.tryQueue(if (filled > 0) encoderInput(pcm, silence, senderMuted.get()) else null, frame * 20_000, end = true)
                    },
                    pollOutput = ::pollEncodedOutput,
                    idle = { Thread.sleep(2) },
                )
            }
            while (running.get()) {
                val now = SystemClock.elapsedRealtime()
                if (now - lastDurationPublication >= 1_000) {
                    lastDurationPublication = now
                    val duration = (now - captureStartMillis) / 1_000
                    handler.post {
                        LabStatus.publishCaptureDuration(duration)
                        updateForegroundDuration(duration)
                    }
                }
                if (filled < pcm.size) {
                    val read = activeRecord.read(pcm, filled, pcm.size - filled, AudioRecord.READ_NON_BLOCKING)
                    check(read >= 0 && read <= pcm.size - filled && read % 2 == 0)
                    captureReadWait.observe(read, SystemClock.elapsedRealtime())
                    if (read > 0) {
                        val transition = signalMonitor.observe(pcm, filled, read)
                        if (transition != null) handler.post {
                            if (running.get() && !terminalStatus) {
                                publishCaptureStatus(when (transition) {
                                    LabSignalTransition.NoCapturableAudio -> {
                                        LabStatus.publishSignal(LabSignalState.Silent)
                                        updateForegroundSignal("No capturable audio detected")
                                        "No capturable audio detected. The source may be silent or capture-limited."
                                    }
                                    LabSignalTransition.SignalDetected -> {
                                        LabStatus.publishSignal(LabSignalState.Detected)
                                        updateForegroundSignal("Capture signal detected")
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
                    if (activeCodec.tryQueue(encoderInput(pcm, silence, senderMuted.get()), frame * 20_000)) {
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
            if (!running.get() && pairedSupervisor != null && !terminalStatus) {
                try {
                    if (flushEncoderTail?.invoke() == false) {
                        tailFailed = true
                        Log.w("SoniMeshLab", "Encoder tail drain timed out")
                    }
                } catch (error: Exception) {
                    tailFailed = true
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
                    val duration = (SystemClock.elapsedRealtime() - captureStartMillis) / 1_000
                    LabStatus.publishCaptureDuration(duration)
                    updateForegroundDuration(duration)
                }
            }
            codec?.close()
            pairedProducerFinished.set(true)
            if (pairedSupervisor != null) {
                val supervisorExited = joinUninterrupted(pairedSupervisor, PAIRED_STOP_TIMEOUT_MS)
                if (!supervisorExited) {
                    pairedSupervisor.interrupt()
                    stopResult = Result.failure(java.io.IOException("paired session supervisor did not stop before timeout"))
                } else {
                    stopResult = pairedSupervisor.stopResult
                }
                stopResult?.exceptionOrNull()?.let { Log.e("SoniMeshLab", "Paired stream stop failed", it) }
                if (activePairedSupervisor === pairedSupervisor) activePairedSupervisor = null
            }
            if (captureStarted) {
                val timing = senderTiming.snapshot()
                handler.post {
                    LabStatus.publishSentPackets(sentPackets.get())
                    LabStatus.publishSenderTiming(timing)
                }
            }
            handler.post {
                workerFinished = true
                captureStopWarning(terminalStatus, stopResult, tailFailed)?.let {
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
        destinationPort: Int,
        producerFinished: AtomicBoolean? = null,
        onSent: () -> Unit = {},
    ) {
        try {
            DatagramSocket().use { udp ->
                udp.connect(destination, destinationPort)
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

    private fun updateForegroundDuration(seconds: Long) {
        val builder = foregroundNotificationBuilder ?: return
        val manager = foregroundNotificationManager ?: return
        builder.setSubText("Capture duration: ${seconds.coerceAtLeast(0)}s")
        manager.notify(1, builder.build())
    }

    private fun updateForegroundActions(muted: Boolean) {
        val builder = foregroundNotificationBuilder ?: return
        val manager = foregroundNotificationManager ?: return
        builder.setActions(stopAction, if (muted) unmuteAction else muteAction)
        updateForegroundContentText(builder)
        manager.notify(1, builder.build())
    }

    private fun publishCaptureStatus(message: String) {
        if (foregroundConnectionStatus != ForegroundConnectionStatus.Reconnecting) LabStatus.publish(message)
    }

    private fun updateForegroundConnectionStatus(message: String?) {
        foregroundConnectionStatus = when {
            message == null -> null
            message.startsWith("Desktop listener is not ready.") -> ForegroundConnectionStatus.Connecting
            message.startsWith("Desktop connection lost.") -> ForegroundConnectionStatus.Reconnecting
            message.startsWith("Desktop connection restored.") -> ForegroundConnectionStatus.Restored
            else -> return
        }
        val builder = foregroundNotificationBuilder ?: return
        val manager = foregroundNotificationManager ?: return
        updateForegroundContentText(builder)
        manager.notify(1, builder.build())
    }

    private fun updateForegroundSignal(label: String) {
        foregroundSignalLabel = label
        val builder = foregroundNotificationBuilder ?: return
        val manager = foregroundNotificationManager ?: return
        updateForegroundContentText(builder)
        manager.notify(1, builder.build())
    }

    private fun updateForegroundContentText(builder: Notification.Builder) {
        val transport = foregroundTransportLabel ?: return
        val (content, expanded) = foregroundNotificationText(
            transport, foregroundConnectionStatus?.label, senderMuted.get(), foregroundSignalLabel,
        )
        builder.setContentText(content)
        builder.setStyle(Notification.BigTextStyle().bigText(expanded))
    }

    private fun requestStop() {
        if (!running.getAndSet(false)) return
        LabStatus.publishSenderMuted(null)
        setupSocket.cancel()
        synchronized(socketLock) {
            try { socket?.close() } catch (_: java.io.IOException) { }
            socket = null
        }
        captureWorker?.interrupt()
        activePairedSupervisor?.cancel()
        if (!terminalStatus) LabStatus.publish("Stopping capture…")
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
            foregroundNotificationManager = null
            foregroundNotificationBuilder = null
            foregroundTransportLabel = null
            foregroundConnectionStatus = null
            stopAction = null
            muteAction = null
            unmuteAction = null
        }
        if (!terminalStatus) {
            LabStatus.publish(if (workerStarted && !workerFinished) "Stopping capture…" else "Capture stopped.")
        }
        destroyed = true
        if (started && (!workerStarted || workerFinished)) LabStatus.serviceFinished()
        super.onDestroy()
    }

    companion object {
        private const val PAIRED_STOP_TIMEOUT_MS = 4_000L
        const val ACTION_SET_MUTED = "com.sonimesh.opusspike.SET_MUTED"
        const val EXTRA_MUTED = "muted"
    }
}

internal fun joinUninterrupted(thread: Thread, timeoutMillis: Long): Boolean {
    require(timeoutMillis >= 0)
    val deadline = SystemClock.elapsedRealtime() + timeoutMillis
    var interrupted = false
    try {
        while (thread.isAlive) {
            val remaining = deadline - SystemClock.elapsedRealtime()
            if (remaining <= 0) return false
            try {
                thread.join(remaining)
            } catch (_: InterruptedException) {
                interrupted = true
            }
        }
        return true
    } finally {
        if (interrupted) Thread.currentThread().interrupt()
    }
}
