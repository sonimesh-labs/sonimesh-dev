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
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.getValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.sonimesh.sender.capture.CaptureService
import com.sonimesh.sender.capture.CaptureEvidenceSnapshot
import com.sonimesh.sender.capture.CaptureStateStore
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.CaptureUiState

class MainActivity : ComponentActivity() {
    private val audioPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) {
                requestNotificationThenProjection()
            } else {
                CaptureStateStore.publishFailure("Microphone permission was not granted")
            }
        }

    private val notificationPermissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) {
                launchProjectionConsent()
            } else {
                CaptureStateStore.publishFailure("Notification permission is required for capture")
            }
        }

    private val projectionLauncher =
        registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
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
        setContent {
            val state by CaptureStateStore.state.collectAsState()
            MaterialTheme {
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
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
            && ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
            != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            launchProjectionConsent()
        }
    }

    private fun launchProjectionConsent() {
        CaptureStateStore.publishAwaitingProjectionConsent()
        val manager = getSystemService(MediaProjectionManager::class.java)
        projectionLauncher.launch(manager.createScreenCaptureIntent())
    }

    private fun sendStopCommand() {
        startService(Intent(this, CaptureService::class.java).setAction(CaptureService.ACTION_STOP))
    }
}

@androidx.compose.runtime.Composable
private fun CaptureScreen(
    state: CaptureUiState,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("SoniMesh capture feasibility", style = MaterialTheme.typography.headlineSmall)
        Text(
            "Playback capture can be silent or policy-limited. This prototype reports observations without identifying a source-specific cause.",
            style = MaterialTheme.typography.bodyMedium,
        )
        Text(state.status.description(), style = MaterialTheme.typography.bodyLarge)
        if (state.evidence.hasObservations()) {
            Text(state.evidence.description(), style = MaterialTheme.typography.bodyMedium)
        }
        if (state.status.isActive()) {
            Button(onClick = onStop) { Text("Stop capture") }
        } else {
            Button(onClick = onStart) { Text("Start capture") }
        }
    }
}

private fun CaptureStatus.isActive(): Boolean = when (this) {
    is CaptureStatus.Capturing, is CaptureStatus.NoCapturableAudio -> true
    else -> false
}

private fun CaptureStatus.description(): String = when (this) {
    CaptureStatus.Idle -> "Ready to request playback capture consent."
    CaptureStatus.AwaitingPermission -> "Waiting for microphone permission."
    CaptureStatus.AwaitingProjectionConsent -> "Waiting for playback capture consent."
    is CaptureStatus.Capturing -> "Capturing ${format.sampleRateHz} Hz, ${format.channels} channels, ${format.bufferSizeBytes} byte buffer."
    is CaptureStatus.NoCapturableAudio -> "No capturable audio detected. The source may be silent or policy-restricted; the cause is not identifiable here."
    is CaptureStatus.Stopped -> "Capture stopped: ${reason.displayName}."
    is CaptureStatus.Failed -> "Capture failed: $message"
}

private fun CaptureEvidenceSnapshot.description(): String =
    "Observed ${capturedFrames} frames, ${silentReads} silent reads, ${readErrors} read errors."

private fun CaptureEvidenceSnapshot.hasObservations(): Boolean =
    capturedFrames != 0L || silentReads != 0L || readErrors != 0L
