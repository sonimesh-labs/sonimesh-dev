package com.sonimesh.sender.capture

import android.Manifest
import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.sonimesh.sender.R
import java.util.concurrent.atomic.AtomicBoolean

class CaptureService : Service() {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val teardownStarted = AtomicBoolean(false)
    private val readerExited = AtomicBoolean(true)

    @Volatile
    private var audioRecord: AudioRecord? = null

    @Volatile
    private var mediaProjection: MediaProjection? = null

    @Volatile
    private var captureThread: Thread? = null

    private var projectionCallback: MediaProjection.Callback? = null

    @Volatile
    private var pendingOutcome: TerminalOutcome? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            teardown(TerminalOutcome.Stopped(CaptureStopReason.UserRequested))
            return START_NOT_STICKY
        }
        if (teardownStarted.get() || audioRecord != null || mediaProjection != null) {
            return START_NOT_STICKY
        }

        val resultCode = intent?.getIntExtra(EXTRA_RESULT_CODE, Activity.RESULT_CANCELED)
            ?: Activity.RESULT_CANCELED
        val resultData = intent?.projectionData()
        if (resultCode != Activity.RESULT_OK || resultData == null) {
            teardown(TerminalOutcome.Failed("Missing playback capture consent result"))
            return START_NOT_STICKY
        }

        try {
            startForegroundServiceNotification()
            startCapture(resultCode, resultData)
        } catch (error: SecurityException) {
            teardown(TerminalOutcome.Failed("Foreground capture service permission was rejected"))
        } catch (error: IllegalStateException) {
            teardown(TerminalOutcome.Failed("Foreground capture service could not start"))
        }
        return START_NOT_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        teardown(TerminalOutcome.Stopped(CaptureStopReason.ServiceDestroyed))
        super.onDestroy()
    }

    private fun startCapture(resultCode: Int, resultData: Intent) {
        var candidateRecord: AudioRecord? = null
        try {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                != PackageManager.PERMISSION_GRANTED
            ) {
                throw CaptureSetupException("Microphone permission is not available")
            }
            val projectionManager = getSystemService(MediaProjectionManager::class.java)
            val projection = projectionManager.getMediaProjection(resultCode, resultData)
                ?: throw CaptureSetupException("MediaProjection could not be created")
            val callback = object : MediaProjection.Callback() {
                override fun onStop() {
                    teardown(TerminalOutcome.Stopped(CaptureStopReason.ProjectionRevoked))
                }
            }
            mediaProjection = projection
            projectionCallback = callback
            projection.registerCallback(callback, mainHandler)

            val record = createAudioRecord(projection).also { candidateRecord = it }
            if (record.state != AudioRecord.STATE_INITIALIZED) {
                throw CaptureSetupException("AudioRecord did not initialize")
            }

            val format = observedFormat(record)
            audioRecord = record
            candidateRecord = null
            CaptureStateStore.evidence.begin(SystemClock.elapsedRealtime())

            record.startRecording()
            CaptureStateStore.publishCapturing(format)
            startCaptureThread(record, format)
        } catch (error: SecurityException) {
            teardown(TerminalOutcome.Failed("Capture permission was rejected by the platform"))
        } catch (error: IllegalArgumentException) {
            teardown(TerminalOutcome.Failed("Capture format was rejected by the platform"))
        } catch (error: IllegalStateException) {
            teardown(TerminalOutcome.Failed("Capture could not start"))
        } catch (error: CaptureSetupException) {
            teardown(TerminalOutcome.Failed(error.message ?: "Capture setup failed"))
        } finally {
            candidateRecord?.let { record -> runCatching { record.release() } }
        }
    }

    @android.annotation.SuppressLint("MissingPermission")
    private fun createAudioRecord(projection: MediaProjection): AudioRecord {
        val captureConfig = android.media.AudioPlaybackCaptureConfiguration.Builder(projection)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
            .addMatchingUsage(AudioAttributes.USAGE_GAME)
            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
            .build()
        val minimumBuffer = AudioRecord.getMinBufferSize(
            REQUESTED_SAMPLE_RATE_HZ,
            AudioFormat.CHANNEL_IN_STEREO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minimumBuffer <= 0) {
            throw CaptureSetupException("Platform returned an invalid AudioRecord buffer size")
        }
        if (minimumBuffer > MAX_CAPTURE_BUFFER_BYTES) {
            throw CaptureSetupException("AudioRecord minimum buffer exceeds the prototype bound")
        }
        val requestedBytes = REQUESTED_READ_SAMPLES * Short.SIZE_BYTES
        val bufferSize = maxOf(minimumBuffer, requestedBytes)

        return AudioRecord.Builder()
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(REQUESTED_SAMPLE_RATE_HZ)
                    .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                    .build(),
            )
            .setBufferSizeInBytes(bufferSize)
            .setAudioPlaybackCaptureConfig(captureConfig)
            .build()
    }

    private fun observedFormat(record: AudioRecord): CaptureFormat {
        val bufferBytes = record.bufferSizeInFrames.toLong()
            .times(record.channelCount.toLong())
            .times(Short.SIZE_BYTES.toLong())
            .takeIf { it in 1..MAX_CAPTURE_BUFFER_BYTES.toLong() }
            ?.toInt()
            ?: throw CaptureSetupException("AudioRecord reported an unbounded buffer")
        return CaptureFormat.fromObserved(
            sampleRateHz = record.sampleRate,
            channels = record.channelCount,
            bufferSizeBytes = bufferBytes,
        ) ?: throw CaptureSetupException("AudioRecord reported an unsupported format")
    }

    private fun startCaptureThread(record: AudioRecord, format: CaptureFormat) {
        readerExited.set(false)
        val thread = Thread({ readCaptureLoop(record, format) }, "sonimesh-capture")
        captureThread = thread
        thread.start()
    }

    private fun readCaptureLoop(record: AudioRecord, format: CaptureFormat) {
        val buffer = ShortArray(MAX_READ_SAMPLES)
        val signalMonitor = CaptureSignalMonitor(SILENT_READ_LIMIT)
        val evidenceThrottle = CaptureEvidencePublishThrottle(
            CAPTURE_EVIDENCE_PUBLISH_INTERVAL_MS,
        )

        try {
            while (!teardownStarted.get()) {
                if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
                    != PackageManager.PERMISSION_GRANTED
                ) {
                    mainHandler.post {
                        if (!teardownStarted.get()) {
                            teardown(TerminalOutcome.Failed("Microphone permission was revoked"))
                        }
                    }
                    return
                }

                val read = try {
                    // Non-blocking reads keep teardown independent of platform read wake-up behavior.
                    record.read(buffer, 0, format.readSamples, AudioRecord.READ_NON_BLOCKING)
                } catch (error: IllegalStateException) {
                    if (!teardownStarted.get()) {
                        mainHandler.post {
                            if (!teardownStarted.get()) {
                                teardown(TerminalOutcome.Failed("AudioRecord read failed"))
                            }
                        }
                    }
                    return
                } catch (error: SecurityException) {
                    if (!teardownStarted.get()) {
                        mainHandler.post {
                            if (!teardownStarted.get()) {
                                teardown(TerminalOutcome.Failed("Microphone permission was revoked"))
                            }
                        }
                    }
                    return
                }
                if (read == 0) {
                    try {
                        Thread.sleep(CAPTURE_POLL_INTERVAL_MS)
                    } catch (error: InterruptedException) {
                        if (!teardownStarted.get()) {
                            mainHandler.post {
                                if (!teardownStarted.get()) {
                                    teardown(TerminalOutcome.Failed("AudioRecord reader was interrupted"))
                                }
                            }
                        }
                        return
                    }
                    continue
                }
                if (read < 0) {
                    CaptureStateStore.evidence.recordReadError()
                    mainHandler.post {
                        if (!teardownStarted.get()) {
                            teardown(TerminalOutcome.Failed("AudioRecord read failed"))
                        }
                    }
                    return
                }
                if (teardownStarted.get()) {
                    return
                }

                var silent = true
                var index = 0
                while (index < read) {
                    if (buffer[index] != 0.toShort()) {
                        silent = false
                        break
                    }
                    index++
                }
                CaptureStateStore.evidence.recordRead(read / format.channels, silent)
                if (evidenceThrottle.shouldPublish(SystemClock.elapsedRealtime())) {
                    mainHandler.post {
                        if (!teardownStarted.get()) {
                            CaptureStateStore.publishEvidence()
                        }
                    }
                }
                when (signalMonitor.observe(silent)) {
                    CaptureSignalTransition.NoCapturableAudio -> {
                        mainHandler.post {
                            if (!teardownStarted.get()) {
                                CaptureStateStore.publishNoCapturableAudio(format)
                            }
                        }
                    }
                    CaptureSignalTransition.SignalDetected -> {
                        mainHandler.post {
                            if (!teardownStarted.get()) {
                                CaptureStateStore.publishCapturing(format)
                            }
                        }
                    }
                    null -> Unit
                }
            }
        } finally {
            readerExited.set(true)
            mainHandler.post { finishTeardownIfReady() }
        }
    }

    private fun startForegroundServiceNotification() {
        val notification = NotificationCompat.Builder(this, NOTIFICATION_CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_btn_speak_now)
            .setContentTitle(getString(R.string.capture_notification_title))
            .setContentText(getString(R.string.capture_notification_text))
            .setOngoing(true)
            .addAction(
                NotificationCompat.Action.Builder(
                    android.R.drawable.ic_menu_close_clear_cancel,
                    getString(R.string.stop_capture),
                    stopPendingIntent(),
                ).build(),
            )
            .build()
        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
        )
    }

    private fun stopPendingIntent(): PendingIntent = PendingIntent.getService(
        this,
        STOP_REQUEST_CODE,
        Intent(this, CaptureService::class.java).setAction(ACTION_STOP),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

    private fun createNotificationChannel() {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL_ID,
                getString(R.string.capture_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    private fun teardown(outcome: TerminalOutcome) {
        if (!teardownStarted.compareAndSet(false, true)) {
            return
        }
        pendingOutcome = outcome

        captureThread?.interrupt()
        audioRecord?.let { record -> runCatching { record.stop() } }
        finishTeardownIfReady()
    }

    private fun finishTeardownIfReady() {
        if (!teardownStarted.get() || !readerExited.get()) {
            return
        }

        val outcome = pendingOutcome ?: return
        captureThread = null

        audioRecord?.let { record -> record.release() }
        audioRecord = null

        mediaProjection?.let { projection ->
            projectionCallback?.let { callback ->
                runCatching { projection.unregisterCallback(callback) }
            }
            runCatching { projection.stop() }
        }
        mediaProjection = null
        projectionCallback = null
        pendingOutcome = null

        when (outcome) {
            is TerminalOutcome.Stopped -> CaptureStateStore.publishStopped(outcome.reason)
            is TerminalOutcome.Failed -> CaptureStateStore.publishFailure(outcome.message)
        }
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private sealed interface TerminalOutcome {
        data class Stopped(val reason: CaptureStopReason) : TerminalOutcome
        data class Failed(val message: String) : TerminalOutcome
    }

    private class CaptureSetupException(message: String) : Exception(message)

    private fun Intent.projectionData(): Intent? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(EXTRA_RESULT_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableExtra(EXTRA_RESULT_DATA)
        }
    }

    companion object {
        const val ACTION_STOP = "com.sonimesh.sender.action.STOP_CAPTURE"
        const val EXTRA_RESULT_CODE = "com.sonimesh.sender.extra.RESULT_CODE"
        const val EXTRA_RESULT_DATA = "com.sonimesh.sender.extra.RESULT_DATA"

        private const val NOTIFICATION_CHANNEL_ID = "sonimesh-capture"
        private const val NOTIFICATION_ID = 1001
        private const val STOP_REQUEST_CODE = 1002
        private const val SILENT_READ_LIMIT = 100
        private const val CAPTURE_POLL_INTERVAL_MS = 5L
        private const val CAPTURE_EVIDENCE_PUBLISH_INTERVAL_MS = 500L
        private const val REQUESTED_SAMPLE_RATE_HZ = 48_000
        private const val REQUESTED_CHANNELS = 2
        private const val REQUESTED_READ_FRAMES = 960
        private const val REQUESTED_READ_SAMPLES = REQUESTED_READ_FRAMES * REQUESTED_CHANNELS
    }
}
