package com.sonimesh.sender

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.getValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import com.sonimesh.sender.capture.CaptureService
import com.sonimesh.sender.capture.CaptureStateStore
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.canStart
import com.sonimesh.sender.capture.isAwaitingAuthorization

class MainActivity : ComponentActivity() {
    private val audioPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (CaptureStateStore.state.value.status != CaptureStatus.AwaitingPermission) {
                return@registerForActivityResult
            }
            if (granted) {
                requestNotificationThenProjection()
            } else {
                CaptureStateStore.publishFailure("Microphone permission was not granted")
            }
        }

    private val notificationPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (CaptureStateStore.state.value.status != CaptureStatus.AwaitingNotificationPermission) {
                return@registerForActivityResult
            }
            if (granted) {
                launchProjectionConsent()
            } else {
                CaptureStateStore.publishFailure("Notification permission is required for capture")
            }
        }

    private val projectionLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
            if (CaptureStateStore.state.value.status != CaptureStatus.AwaitingProjectionConsent) {
                return@registerForActivityResult
            }
            if (result.resultCode != Activity.RESULT_OK || result.data == null) {
                CaptureStateStore.publishFailure("Screen capture consent was not granted")
                return@registerForActivityResult
            }

            val serviceIntent = Intent(this, CaptureService::class.java)
                .putExtra(CaptureService.EXTRA_RESULT_CODE, result.resultCode)
                .putExtra(CaptureService.EXTRA_RESULT_DATA, result.data)
            try {
                ContextCompat.startForegroundService(this, serviceIntent)
            } catch (error: SecurityException) {
                CaptureStateStore.publishFailure("Capture service permission was rejected")
            } catch (error: IllegalStateException) {
                CaptureStateStore.publishFailure("Capture service could not start in the current state")
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val state by CaptureStateStore.state.collectAsState()
            SoniMeshTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    CaptureScreen(
                        state = state,
                        onStart = ::requestCapture,
                        onStop = { sendStopCommand() },
                    )
                }
            }
        }
    }

    private fun requestCapture() {
        if (!CaptureStateStore.state.value.status.canStart()) {
            return
        }

        CaptureStateStore.publishAwaitingPermission()
        if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO)
            != PackageManager.PERMISSION_GRANTED
        ) {
            audioPermissionLauncher.launch(Manifest.permission.RECORD_AUDIO)
        } else {
            requestNotificationThenProjection()
        }
    }

    private fun requestNotificationThenProjection() {
        if (CaptureStateStore.state.value.status != CaptureStatus.AwaitingPermission) {
            return
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
            && ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            CaptureStateStore.publishAwaitingNotificationPermission()
            notificationPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            launchProjectionConsent()
        }
    }

    private fun launchProjectionConsent() {
        if (!CaptureStateStore.state.value.status.isAwaitingAuthorization()) {
            return
        }
        CaptureStateStore.publishAwaitingProjectionConsent()
        val manager = getSystemService(MediaProjectionManager::class.java)
        projectionLauncher.launch(manager.createScreenCaptureIntent())
    }

    private fun sendStopCommand() {
        startService(Intent(this, CaptureService::class.java).setAction(CaptureService.ACTION_STOP))
    }
}


internal fun CaptureStatus.description(): String = when (this) {
    CaptureStatus.Idle -> "Ready to request playback capture consent."
    CaptureStatus.AwaitingPermission -> "Waiting for microphone permission."
    CaptureStatus.AwaitingNotificationPermission -> "Waiting for notification permission."
    CaptureStatus.AwaitingProjectionConsent -> "Waiting for playback capture consent."
    is CaptureStatus.Capturing -> "Capturing ${format.description()}."
    is CaptureStatus.NoCapturableAudio ->
        "No capturable audio detected. ${format.description()}. The source may be silent or policy-restricted; the cause is not identifiable here."
    is CaptureStatus.Stopped -> "Capture stopped: ${reason.displayName}."
    is CaptureStatus.Failed -> "Capture failed: $message"
}

private fun com.sonimesh.sender.capture.CaptureFormat.description(): String =
    "${sampleRateHz} Hz, ${channels} channels, source ${audioSource}, channel mask 0x${channelMask.toString(16)}, ${bufferSizeBytes} byte buffer (platform minimum ${minimumBufferBytes})"

internal fun com.sonimesh.sender.capture.CaptureEvidenceSnapshot.description(): String {
    val latestRead = lastReadCompletionOffsetMillis?.let { offset ->
        " Last successful read at +${formatDuration(offset)}."
    }.orEmpty()
    return "Observed ${capturedFrames} frames over ${formatDuration(captureDurationMillis)}, " +
        "${silentFrames} silent frames across ${silentReads} reads, " +
        "${readErrors} read errors.$latestRead"
}

internal fun com.sonimesh.sender.capture.CaptureEvidenceSnapshot.hasObservations(): Boolean =
    captureDurationMillis != 0L || capturedFrames != 0L || silentFrames != 0L ||
        silentReads != 0L || readErrors != 0L

internal fun formatDuration(durationMillis: Long): String =
    "${durationMillis / 1_000}.${(durationMillis % 1_000) / 100} s"
