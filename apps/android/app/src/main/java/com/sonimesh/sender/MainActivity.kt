package com.sonimesh.sender

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import com.sonimesh.sender.capture.CaptureService
import com.sonimesh.sender.capture.CaptureSource
import com.sonimesh.sender.capture.CaptureStateStore
import com.sonimesh.sender.capture.CaptureStatus
import com.sonimesh.sender.capture.canStart
import com.sonimesh.sender.capture.isActive
import com.sonimesh.sender.capture.isAwaitingAuthorization
import java.util.Locale

class MainActivity : ComponentActivity() {
    private var pendingCaptureSource = CaptureSource.Media
    private val testTone = CaptureTestTone()
    private var testTonePlaying by mutableStateOf(false)
    private val usageCheckHandler = Handler(Looper.getMainLooper())
    private var usageCheckRunnable: Runnable? = null
    private var usageCheckSources = emptyList<CaptureSource>()
    private val usageCheckResults = mutableListOf<String>()
    private var usageCheckRunning by mutableStateOf(false)
    private var usageCheckSummary by mutableStateOf("")
    private var usageCheckEvidenceSummary = ""

    override fun onCreate(savedInstanceState: Bundle?) {
        pendingCaptureSource = CaptureSource.fromWireValue(
            savedInstanceState?.getString(STATE_PENDING_CAPTURE_SOURCE),
        )
        usageCheckEvidenceSummary = savedInstanceState
            ?.getString(STATE_USAGE_CHECK_EVIDENCE_SUMMARY)
            .orEmpty()
        usageCheckSummary = savedInstanceState
            ?.getString(STATE_USAGE_CHECK_SUMMARY)
            .orEmpty()
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val state by CaptureStateStore.state.collectAsState()
            LaunchedEffect(state.status.isActive()) {
                if (!state.status.isActive()) {
                    cancelUsageCheck()
                    stopTestTone()
                }
            }
            var selectedSourceWire by rememberSaveable {
                mutableStateOf(
                    (state.evidence.captureSource.takeIf { state.status.isActive() } ?: CaptureSource.Media).wireValue,
                )
            }
            val selectedSource = CaptureSource.fromWireValue(selectedSourceWire)
            SoniMeshTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    CaptureScreen(
                        state = state,
                        selectedSource = state.evidence.captureSource.takeIf { state.status.isActive() } ?: selectedSource,
                        onSourceSelected = { selectedSourceWire = it.wireValue },
                        onStart = { requestCapture(CaptureSource.fromWireValue(selectedSourceWire)) },
                        onStop = { sendStopCommand() },
                        onCopyEvidence = ::copyCaptureEvidence,
                        onShareEvidence = ::shareCaptureEvidence,
                        testTonePlaying = testTonePlaying,
                        onToggleTestTone = ::toggleTestTone,
                        usageCheckRunning = usageCheckRunning,
                        usageCheckSummary = usageCheckSummary,
                        onRunUsageCheck = ::runUsageCheck,
                    )
                }
            }
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString(STATE_PENDING_CAPTURE_SOURCE, pendingCaptureSource.wireValue)
        outState.putString(STATE_USAGE_CHECK_EVIDENCE_SUMMARY, usageCheckEvidenceSummary)
        outState.putString(
            STATE_USAGE_CHECK_SUMMARY,
            if (usageCheckRunning) getString(R.string.capture_usage_check_stopped)
            else usageCheckSummary,
        )
        super.onSaveInstanceState(outState)
    }

    override fun onStop() {
        cancelUsageCheck()
        stopTestTone()
        super.onStop()
    }

    private fun stopTestTone() {
        testTone.close()
        testTonePlaying = false
    }

    private fun toggleTestTone() {
        if (usageCheckRunning) cancelUsageCheck()
        if (testTonePlaying) {
            stopTestTone()
            return
        }
        val state = CaptureStateStore.state.value
        if (!state.status.isActive()) return
        testTonePlaying = testTone.start(state.evidence.captureSource ?: CaptureSource.Media)
        if (!testTonePlaying) Toast.makeText(this, R.string.capture_test_tone_failed, Toast.LENGTH_SHORT).show()
    }

    private fun runUsageCheck() {
        if (usageCheckRunning) {
            cancelUsageCheck()
            return
        }
        if (!CaptureStateStore.state.value.status.isActive()) return
        stopTestTone()
        usageCheckResults.clear()
        usageCheckSources = listOf(CaptureSource.Media, CaptureSource.Game, CaptureSource.Unknown)
        usageCheckSummary = getString(R.string.capture_usage_check_running)
        usageCheckRunning = true
        runNextUsageCheck()
    }

    private fun runNextUsageCheck() {
        val source = usageCheckSources.firstOrNull()
        if (source == null) {
            usageCheckRunning = false
            usageCheckSummary = getString(
                R.string.capture_usage_check_summary,
                usageCheckResults.joinToString("; "),
            )
            usageCheckEvidenceSummary = usageCheckSummary
            return
        }
        usageCheckSources = usageCheckSources.drop(1)
        if (!CaptureStateStore.state.value.status.isActive()) {
            cancelUsageCheck()
            return
        }

        val beforeFrames = nonSilentFrames()
        if (!testTone.start(source)) {
            usageCheckResults += getString(R.string.capture_usage_check_tone_unavailable, source.displayName)
            usageCheckSummary = getString(
                R.string.capture_usage_check_summary,
                usageCheckResults.joinToString("; "),
            )
            scheduleNextUsageCheck()
            return
        }
        testTonePlaying = true
        val startedAt = SystemClock.elapsedRealtime()
        val checkTone = object : Runnable {
            override fun run() {
                if (!usageCheckRunning) return
                if (SystemClock.elapsedRealtime() - startedAt < USAGE_CHECK_TONE_DURATION_MILLIS) {
                    usageCheckHandler.postDelayed(this, USAGE_CHECK_POLL_MILLIS)
                    return
                }
                usageCheckRunnable = null
                val format = CaptureStateStore.state.value.evidence.captureFormat
                val requiredFrames = (format?.sampleRateHz ?: 8_000) / 50
                val detected = nonSilentFrames() - beforeFrames >= requiredFrames
                stopTestTone()
                usageCheckResults += getString(
                    if (detected) R.string.capture_usage_check_detected
                    else R.string.capture_usage_check_not_detected,
                    source.displayName,
                )
                usageCheckSummary = getString(
                    R.string.capture_usage_check_summary,
                    usageCheckResults.joinToString("; "),
                )
                scheduleNextUsageCheck()
            }
        }
        usageCheckRunnable = checkTone
        usageCheckHandler.postDelayed(checkTone, USAGE_CHECK_POLL_MILLIS)
    }

    private fun scheduleNextUsageCheck() {
        if (usageCheckSources.isEmpty()) {
            usageCheckRunning = false
            usageCheckEvidenceSummary = usageCheckSummary
            return
        }
        val next = Runnable {
            usageCheckRunnable = null
            runNextUsageCheck()
        }
        usageCheckRunnable = next
        usageCheckHandler.postDelayed(next, USAGE_CHECK_GAP_MILLIS)
    }

    private fun cancelUsageCheck() {
        if (!usageCheckRunning) return
        usageCheckRunnable?.let(usageCheckHandler::removeCallbacks)
        usageCheckRunnable = null
        usageCheckSources = emptyList()
        usageCheckRunning = false
        stopTestTone()
        usageCheckSummary = getString(R.string.capture_usage_check_stopped)
    }

    private fun nonSilentFrames(): Long {
        val evidence = CaptureStateStore.state.value.evidence
        return evidence.capturedFrames - evidence.silentFrames
    }

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
                .putExtra(CaptureService.EXTRA_CAPTURE_SOURCE, pendingCaptureSource.wireValue)
            try {
                ContextCompat.startForegroundService(this, serviceIntent)
            } catch (error: SecurityException) {
                CaptureStateStore.publishFailure("Capture service permission was rejected")
            } catch (error: IllegalStateException) {
                CaptureStateStore.publishFailure("Capture service could not start in the current state")
            }
        }

    private fun requestCapture(source: CaptureSource) {
        if (!CaptureStateStore.state.value.status.canStart()) {
            return
        }

        usageCheckEvidenceSummary = ""
        usageCheckSummary = ""
        pendingCaptureSource = source
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
        cancelUsageCheck()
        stopTestTone()
        startService(Intent(this, CaptureService::class.java).setAction(CaptureService.ACTION_STOP))
    }

    private fun copyCaptureEvidence() {
        val evidence = CaptureStateStore.state.value.copyableEvidence(
            usageCheckEvidenceSummary.takeIf { it.isNotBlank() },
        )
        val clipboard = getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(
            ClipData.newPlainText(getString(R.string.capture_evidence_clip_label), evidence),
        )
        Toast.makeText(this, R.string.capture_evidence_copied, Toast.LENGTH_SHORT).show()
    }

    private fun shareCaptureEvidence() {
        val evidence = CaptureStateStore.state.value.copyableEvidence(
            usageCheckEvidenceSummary.takeIf { it.isNotBlank() },
        )
        try {
            startActivity(
                Intent.createChooser(
                    buildCaptureEvidenceShareIntent(captureEvidenceShareContent(evidence)),
                    getString(R.string.share_capture_evidence_title),
                ),
            )
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(this, R.string.capture_evidence_share_unavailable, Toast.LENGTH_SHORT).show()
        }
    }

    private companion object {
        const val STATE_PENDING_CAPTURE_SOURCE = "pending_capture_source"
        const val STATE_USAGE_CHECK_EVIDENCE_SUMMARY = "usage_check_evidence_summary"
        const val STATE_USAGE_CHECK_SUMMARY = "usage_check_summary"
        const val USAGE_CHECK_TONE_DURATION_MILLIS = 750L
        const val USAGE_CHECK_POLL_MILLIS = 100L
        const val USAGE_CHECK_GAP_MILLIS = 250L
    }
}

internal data class CaptureEvidenceShareContent(
    val mimeType: String,
    val text: String,
)

internal fun captureEvidenceShareContent(evidence: String): CaptureEvidenceShareContent =
    CaptureEvidenceShareContent(mimeType = "text/plain", text = evidence)

internal fun buildCaptureEvidenceShareIntent(content: CaptureEvidenceShareContent): Intent =
    Intent(Intent.ACTION_SEND).apply {
        type = content.mimeType
        putExtra(Intent.EXTRA_TEXT, content.text)
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

internal fun com.sonimesh.sender.capture.CaptureFormat.description(): String =
    "${sampleRateHz} Hz, ${channels} channels, source ${audioSource}, channel mask 0x${channelMask.toString(16)}, ${bufferSizeBytes} byte buffer (platform minimum ${minimumBufferBytes})"

internal fun com.sonimesh.sender.capture.CaptureEvidenceSnapshot.description(): String {
    val source = captureSource?.displayName?.let { " Requested usage: $it." }.orEmpty()
    val latestRead = lastReadCompletionOffsetMillis?.let { offset ->
        " Last successful read at +${formatDuration(offset)}."
    }.orEmpty()
    val peak = peakSampleMagnitude?.let { " Peak sample level: ${formatCapturePeak(it)}." }.orEmpty()
    return "Observed ${capturedFrames} frames over ${formatDuration(captureDurationMillis)}, " +
        "${silentFrames} silent frames across ${silentReads} reads, " +
        "${readErrors} read errors.$source$latestRead$peak"
}

internal fun formatCapturePeak(magnitude: Int): String =
    String.format(Locale.US, "%.3f%%", magnitude * 100.0 / 32_768)

internal fun com.sonimesh.sender.capture.CaptureUiState.copyableEvidence(
    usageCheckSummary: String? = null,
): String {
    val terminalFormat = if (status is CaptureStatus.Stopped || status is CaptureStatus.Failed) {
        evidence.captureFormat?.let { "\nCapture format: ${it.description()}." }.orEmpty()
    } else {
        ""
    }
    val usageCheck = usageCheckSummary?.let { "\n$it" }.orEmpty()
    return "SoniMesh capture evidence\nStatus: ${status.description()}$terminalFormat\n${evidence.description()}$usageCheck"
}

internal fun com.sonimesh.sender.capture.CaptureEvidenceSnapshot.hasObservations(): Boolean =
    captureFormat != null || captureDurationMillis != 0L || capturedFrames != 0L || silentFrames != 0L ||
        silentReads != 0L || readErrors != 0L

internal fun formatDuration(durationMillis: Long): String =
    "${durationMillis / 1_000}.${(durationMillis % 1_000) / 100} s"
