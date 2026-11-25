package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.PairedConnection
import com.sonimesh.sender.pairing.PairingClient
import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import com.sonimesh.sender.pairing.StreamParameters
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.SocketTimeoutException
import java.security.SecureRandom
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import android.os.SystemClock
import android.util.Log
import java.util.concurrent.atomic.AtomicInteger

internal data class EncodedPacket(val sequence: Long, val bytes: ByteArray)

internal class PairedSessionSupervisor(
    private val running: AtomicBoolean,
    private val producerFinished: AtomicBoolean,
    private val packets: ArrayBlockingQueue<EncodedPacket>,
    private val invitation: ByteArray,
    private val destination: Inet4Address,
    private val destinationPort: Int,
    private val setupSocket: SetupSocket,
    private val onStatus: (String) -> Unit,
    private val onReconnectFailure: (String) -> Unit,
    private val onPacketSent: () -> Unit,
) : Thread("audio-lab-paired-session") {
    private val traceId = nextTraceId.incrementAndGet()
    private val startupComplete = CountDownLatch(1)

    @Volatile
    private var startupError: Exception? = null

    @Volatile
    private var hasStartedSession = false

    @Volatile
    private var activeSocket: java.net.Socket? = null

    @Volatile
    var stopResult: Result<StreamControlResult>? = null
        private set

    fun awaitStarted() {
        startupComplete.await()
        startupError?.let { throw it }
    }

    fun cancel() {
        val socket = activeSocket
        if (socket == null) {
            interrupt()
            return
        }
        Thread({
            try {
                Thread.sleep(STOP_SOCKET_CLOSE_GRACE_MS)
                if (isAlive && !running.get()) runCatching { socket.close() }
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
            }
        }, "audio-lab-control-cancel").apply {
            isDaemon = true
            start()
        }
    }

    override fun run() {
        var reconnectFailures = 0
        try {
            while (running.get()) {
                if (hasStartedSession) {
                    onStatus("Desktop connection lost. Reconnecting while capture continues…")
                    if (!waitBeforeReconnect((reconnectFailures - 1).coerceAtLeast(0))) break
                }

                try {
                    runSession()
                    reconnectFailures = 0
                } catch (error: Exception) {
                    Log.i(TAG, "paired[$traceId] session ended (${error.javaClass.simpleName})")
                    if (!hasStartedSession) {
                        startupError = error
                        return
                    }
                    if (!running.get()) {
                        if (stopResult == null) {
                            stopResult = Result.failure(IOException("control connection closed before the desktop confirmed stop"))
                        }
                        break
                    }
                    reconnectFailures += 1
                    if (reconnectFailures >= MAX_RECONNECT_ATTEMPTS) {
                        onReconnectFailure("Desktop connection was not restored after $MAX_RECONNECT_ATTEMPTS attempts. Stop and start capture to try again.")
                        break
                    }
                }
            }
        } finally {
            if (!hasStartedSession && startupError == null && running.get()) {
                startupError = IOException("paired session ended before startup completed")
            }
            startupComplete.countDown()
            invitation.fill(0)
        }
    }

    private fun runSession() {
        val connection = connect()
        activeSocket = connection.socket
        if (!running.get()) {
            activeSocket = null
            connection.close()
            return
        }
        var identity: StreamIdentity? = null
        val sessionActive = AtomicBoolean(true)
        var sender: Thread? = null
        try {
            Log.i(TAG, "paired[$traceId] control connected")
            connection.negotiateHello("android audio lab", listOf(2))
            Log.i(TAG, "paired[$traceId] hello complete")
            val session = nextSessionId()
            val streamIdentity = StreamIdentity(session, 1, 0)
            identity = streamIdentity
            var startAttempt = 0
            requirePairedStart(
                startPairedWithRetry(
                    running::get,
                    Thread::sleep,
                    onRetry = { Log.i(TAG, "paired[$traceId] retrying StreamStart") },
                ) {
                    startAttempt += 1
                    val result = connection.startStream(StreamParameters(streamIdentity, 48_000, 2, 20))
                    when (result) {
                        is StreamControlResult.Rejected -> Log.i(
                            TAG,
                            "paired[$traceId] StreamStart[$startAttempt] rejected code=${result.code} retryable=${result.retryable} reason=${rejectionReason(result.detail)}",
                        )
                        is StreamControlResult.Status -> Log.i(
                            TAG,
                            "paired[$traceId] StreamStart[$startAttempt] status=${result.state}",
                        )
                    }
                    result
                },
            )
            check(setupSocket.handoff()) { "capture stopped while starting stream" }
            check(running.get()) { "capture stopped while starting stream" }

            val sealer = LabMediaSealer(connection.mediaRoot, session, 1, 0)
            packets.clear()
            sender = Thread({
                sendMedia(sessionActive, sealer)
            }, "audio-lab-paired-sender").also { it.start() }
            connection.socket.soTimeout = CONTROL_POLL_TIMEOUT_MS
            if (hasStartedSession) {
                onStatus("Desktop connection restored. Sending captured audio again.")
            } else {
                hasStartedSession = true
                startupComplete.countDown()
            }
            Log.i(TAG, "paired[$traceId] media session started")

            while (running.get() && sessionActive.get()) {
                try {
                    when (connection.socket.inputStream.read()) {
                        -1 -> throw IOException("desktop closed the control connection")
                        else -> throw IOException("unexpected desktop control data")
                    }
                } catch (_: SocketTimeoutException) {
                    // The control channel has no messages while capture is running.
                }
            }

            if (running.get()) throw IOException("paired media sender ended")

            if (!running.get() && sessionActive.get()) {
                sender?.join(SENDER_DRAIN_TIMEOUT_MS)
                if (sender?.isAlive == true) sender?.interrupt()
                stopResult = attemptPairedStop(identity, false) { stream ->
                    connection.stopStream(stream, 2_000)
                }
            }
        } finally {
            sessionActive.set(false)
            sender?.join(SENDER_EXIT_TIMEOUT_MS)
            if (sender?.isAlive == true) sender?.interrupt()
            if (activeSocket === connection.socket) activeSocket = null
            connection.close()
        }
    }

    private fun connect(): PairedConnection {
        var connectAttempt = 0
        val connection = connectPairedWithRetry(running::get, Thread::sleep, onRetry = {
            Log.i(TAG, "paired[$traceId] retrying control connection")
            onStatus("Desktop listener is not ready. Retrying connection…")
        }) {
            connectAttempt += 1
            Log.i(TAG, "paired[$traceId] control connection attempt $connectAttempt")
            val raw = java.net.Socket()
            check(setupSocket.track(raw)) { "capture stopped before pairing" }
            PairingClient.connect(invitation, raw).also {
                Log.i(TAG, "paired[$traceId] connected remote=${it.socket.remoteSocketAddress}")
            }
        }
        try {
            check(running.get()) { "capture stopped during pairing" }
            return connection
        } catch (error: Exception) {
            connection.close()
            throw error
        }
    }

    private fun sendMedia(sessionActive: AtomicBoolean, sealer: LabMediaSealer) {
        try {
            DatagramSocket().use { udp ->
                udp.connect(destination, destinationPort)
                while (sessionActive.get() && (!producerFinished.get() || packets.isNotEmpty())) {
                    val packet = packets.poll(20, TimeUnit.MILLISECONDS) ?: continue
                    if (!sessionActive.get()) break
                    val datagram = sealer.seal(packet.sequence, packet.sequence * 960, 0, packet.bytes)
                    udp.send(DatagramPacket(datagram, datagram.size))
                    onPacketSent()
                }
            }
        } catch (_: Exception) {
            sessionActive.set(false)
        } finally {
            sealer.clear()
        }
    }

    private fun waitBeforeReconnect(failures: Int): Boolean {
        val delay = pairedReconnectDelayMillis(failures)
        val deadline = SystemClock.elapsedRealtime() + delay
        while (running.get() && SystemClock.elapsedRealtime() < deadline) {
            try {
                Thread.sleep(minOf(100L, deadline - SystemClock.elapsedRealtime()))
            } catch (_: InterruptedException) {
                Thread.currentThread().interrupt()
                return false
            }
        }
        return running.get()
    }

    private fun nextSessionId(): Long {
        val random = SecureRandom()
        var session = random.nextLong()
        while (session == 0L) session = random.nextLong()
        return session
    }

    private companion object {
        const val TAG = "SoniMeshLab"
        val nextTraceId = AtomicInteger()
        const val MAX_RECONNECT_ATTEMPTS = 12
        const val MAX_RECONNECT_DELAY_MS = 4_000L
        const val CONTROL_POLL_TIMEOUT_MS = 250
        const val SENDER_DRAIN_TIMEOUT_MS = 2_500L
        const val SENDER_EXIT_TIMEOUT_MS = 500L
        const val STOP_SOCKET_CLOSE_GRACE_MS = 3_500L
    }

    private fun rejectionReason(detail: String): String = when (detail) {
        "test-injected receiver unavailable" -> "test-injected"
        "output unavailable" -> "output-unavailable"
        else -> "other"
    }
}

internal fun pairedReconnectDelayMillis(failures: Int): Long =
    minOf(500L shl failures.coerceIn(0, 3), 4_000L)
