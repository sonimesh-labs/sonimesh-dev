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
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import com.sonimesh.sender.R
import java.util.concurrent.atomic.AtomicBoolean

class CaptureService : Service() {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val teardownStarted = AtomicBoolean(false)
    private val noSignalPublished = AtomicBoolean(false)

    @Volatile
    private var audioRecord: AudioRecord? = null

    @Volatile
    private var mediaProjection: MediaProjection? = null

    @Volatile
    private var captureThread: Thread? = null

    private var projectionCallback: MediaProjection.Callback? = null
    private var activeFormat: CaptureFormat? = null

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

            val record = createAudioRecord(projection)
            if (record.state != AudioRecord.STATE_INITIALIZED) {
                record.release()
                throw CaptureSetupException("AudioRecord did not initialize")
            }

            audioRecord = record
            activeFormat = CAPTURE_FORMAT.copy(
                bufferSizeBytes = record.bufferSizeInFrames
                    * CAPTURE_FORMAT.channels
                    * Short.SIZE_BYTES,
            )
            CaptureStateStore.evidence.reset()
            noSignalPublished.set(false)

            record.startRecording()
            CaptureStateStore.publishCapturing(activeFormat!!)
            startCaptureThread(record, activeFormat!!)
        } catch (error: SecurityException) {
            teardown(TerminalOutcome.Failed("Capture permission was rejected by the platform"))
        } catch (error: IllegalArgumentException) {
            teardown(TerminalOutcome.Failed("Capture format was rejected by the platform"))
        } catch (error: IllegalStateException) {
            teardown(TerminalOutcome.Failed("Capture could not start"))
        } catch (error: CaptureSetupException) {
            teardown(TerminalOutcome.Failed(error.message ?: "Capture setup failed"))
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
            CAPTURE_FORMAT.sampleRateHz,
            AudioFormat.CHANNEL_IN_STEREO,
            AudioFormat.ENCODING_PCM_16BIT,
        )
        if (minimumBuffer <= 0) {
            throw CaptureSetupException("Platform returned an invalid AudioRecord buffer size")
        }
        val requestedBytes = READ_SAMPLES * Short.SIZE_BYTES
        val bufferSize = maxOf(minimumBuffer, requestedBytes).coerceAtMost(MAX_BUFFER_BYTES)
        if (bufferSize < requestedBytes) {
            throw CaptureSetupException("AudioRecord buffer exceeds the prototype bound")
        }

        return AudioRecord.Builder()
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(CAPTURE_FORMAT.sampleRateHz)
                    .setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
                    .build(),
            )
            .setBufferSizeInBytes(bufferSize)
            .setAudioPlaybackCaptureConfig(captureConfig)
            .build()
    }

    private fun startCaptureThread(record: AudioRecord, format: CaptureFormat) {
        val thread = Thread({ readCaptureLoop(record, format) }, "sonimesh-capture")
        captureThread = thread
        thread.start()
    }

    private fun readCaptureLoop(record: AudioRecord, format: CaptureFormat) {
        val buffer = ShortArray(READ_SAMPLES)
        var consecutiveSilentReads = 0

        while (!teardownStarted.get()) {
            val read = record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
            if (read <= 0) {
                CaptureStateStore.evidence.recordReadError()
                mainHandler.post {
                    teardown(TerminalOutcome.Failed("AudioRecord read failed"))
                }
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
            if (silent) {
                consecutiveSilentReads++
                if (consecutiveSilentReads == SILENT_READ_LIMIT
                    && noSignalPublished.compareAndSet(false, true)
                ) {
                    mainHandler.post {
                        CaptureStateStore.publishNoCapturableAudio(format)
                    }
                }
            } else {
                consecutiveSilentReads = 0
                if (noSignalPublished.compareAndSet(true, false)) {
                    mainHandler.post {
                        CaptureStateStore.publishCapturing(format)
                    }
                }
            }
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

        captureThread?.interrupt()
        captureThread = null

        audioRecord?.let { record ->
            runCatching { record.stop() }
            record.release()
        }
        audioRecord = null

        mediaProjection?.let { projection ->
            projectionCallback?.let { callback ->
                runCatching { projection.unregisterCallback(callback) }
            }
            runCatching { projection.stop() }
        }
        mediaProjection = null
        projectionCallback = null
        activeFormat = null

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
        private const val READ_FRAMES = 480
        private const val CHANNELS = 2
        private const val READ_SAMPLES = READ_FRAMES * CHANNELS
        private const val MAX_BUFFER_BYTES = 64 * 1024
        private val CAPTURE_FORMAT = CaptureFormat(
            sampleRateHz = 48_000,
            channels = CHANNELS,
            bufferSizeBytes = 0,
        )
    }
}
