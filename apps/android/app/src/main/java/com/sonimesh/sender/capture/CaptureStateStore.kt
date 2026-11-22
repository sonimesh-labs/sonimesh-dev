package com.sonimesh.sender.capture

import android.os.SystemClock
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

internal class CaptureEvidence(
    private val capturedFrames: CaptureEvidenceCounter = SaturatingAtomicCounter(),
    private val silentFrames: CaptureEvidenceCounter = SaturatingAtomicCounter(),
    private val silentReads: CaptureEvidenceCounter = SaturatingAtomicCounter(),
    private val readErrors: CaptureEvidenceCounter = SaturatingAtomicCounter(),
) {
    private val captureStartedAtMillis = AtomicLong(NO_CAPTURE_START)
    private val lastReadCompletedAtMillis = AtomicLong(NO_READ_COMPLETION)

    @Volatile
    private var captureSource: CaptureSource? = null

    /** Records one successful read and its local monotonic completion time. */
    fun recordRead(frames: Int, silent: Boolean, completedAtMillis: Long) {
        require(frames > 0) { "completed read must contain at least one frame" }
        require(completedAtMillis >= 0) { "read completion time must be monotonic" }
        capturedFrames.add(frames)
        if (silent) {
            silentFrames.add(frames)
            silentReads.increment()
        }
        val startedAtMillis = captureStartedAtMillis.get()
        if (startedAtMillis != NO_CAPTURE_START && completedAtMillis >= startedAtMillis) {
            recordLatestReadCompletion(completedAtMillis)
        }
    }

    /** Records one non-teardown AudioRecord read failure. */
    fun recordReadError() {
        readErrors.increment()
    }

    /** Starts a fresh evidence interval at the supplied monotonic timestamp. */
    fun begin(captureStartedAtMillis: Long, captureSource: CaptureSource? = null) {
        require(captureStartedAtMillis >= 0) { "capture start time must be monotonic" }
        reset()
        this.captureSource = captureSource
        this.captureStartedAtMillis.set(captureStartedAtMillis)
    }

    fun reset() {
        captureStartedAtMillis.set(NO_CAPTURE_START)
        lastReadCompletedAtMillis.set(NO_READ_COMPLETION)
        captureSource = null
        capturedFrames.reset()
        silentFrames.reset()
        silentReads.reset()
        readErrors.reset()
    }

    /** Returns a low-rate summary at the supplied monotonic timestamp. */
    fun snapshot(nowMillis: Long): CaptureEvidenceSnapshot {
        require(nowMillis >= 0) { "snapshot time must be monotonic" }
        val startedAtMillis = captureStartedAtMillis.get()
        val captureDurationMillis = if (startedAtMillis == NO_CAPTURE_START) {
            0
        } else {
            (nowMillis - startedAtMillis).coerceAtLeast(0)
        }
        val lastReadCompletionOffsetMillis = lastReadCompletedAtMillis.get()
            .takeIf { completedAtMillis ->
                startedAtMillis != NO_CAPTURE_START && completedAtMillis >= startedAtMillis
            }
            ?.minus(startedAtMillis)
        // Read dependent monotonic counters in reverse write order so a concurrent
        // completed read cannot expose more silent reads than frames or more silent
        // frames than total frames in one low-rate snapshot.
        val observedSilentReads = silentReads.get()
        val observedSilentFrames = silentFrames.get()
        val observedCapturedFrames = capturedFrames.get()
        return CaptureEvidenceSnapshot(
            captureDurationMillis = captureDurationMillis,
            captureSource = captureSource.takeIf { startedAtMillis != NO_CAPTURE_START },
            lastReadCompletionOffsetMillis = lastReadCompletionOffsetMillis,
            capturedFrames = observedCapturedFrames,
            silentFrames = observedSilentFrames,
            silentReads = observedSilentReads,
            readErrors = readErrors.get(),
        )
    }

    private fun recordLatestReadCompletion(completedAtMillis: Long) {
        while (true) {
            val previous = lastReadCompletedAtMillis.get()
            if (completedAtMillis <= previous
                || lastReadCompletedAtMillis.compareAndSet(previous, completedAtMillis)
            ) {
                return
            }
        }
    }

    private companion object {
        const val NO_CAPTURE_START = -1L
        const val NO_READ_COMPLETION = -1L
    }
}

object CaptureStateStore {
    private val mutableState = MutableStateFlow(CaptureUiState())

    val state: StateFlow<CaptureUiState> = mutableState.asStateFlow()
    internal val evidence = CaptureEvidence()

    /** Injectable monotonic clock for deterministic host JVM tests. */
    internal var nowMillis: () -> Long = SystemClock::elapsedRealtime

    fun publishAwaitingPermission() {
        evidence.reset()
        publish(CaptureStatus.AwaitingPermission)
    }

    fun publishAwaitingNotificationPermission() {
        publish(CaptureStatus.AwaitingNotificationPermission)
    }

    fun publishAwaitingProjectionConsent() {
        publish(CaptureStatus.AwaitingProjectionConsent)
    }

    /** Replaces only the fresh consent state rejected by a retiring service. */
    fun publishRetiringServiceStartFailure() {
        val failure = mutableState.value.status.retiringServiceStartFailure() ?: return
        publish(failure)
    }

    fun publishCapturing(format: CaptureFormat) {
        publish(CaptureStatus.Capturing(format))
    }

    fun publishNoCapturableAudio(format: CaptureFormat) {
        publish(CaptureStatus.NoCapturableAudio(format))
    }

    fun publishStopped(reason: CaptureStopReason) {
        publish(CaptureStatus.Stopped(reason))
    }

    fun publishFailure(message: String) {
        publish(CaptureStatus.Failed(boundedCaptureErrorDetail(message)))
    }

    /** Publishes the latest bounded evidence only while capture remains active. */
    fun publishEvidence() {
        val current = mutableState.value
        if (current.status !is CaptureStatus.Capturing
            && current.status !is CaptureStatus.NoCapturableAudio
        ) {
            return
        }
        mutableState.value = current.copy(evidence = evidence.snapshot(nowMillis()))
    }

    private fun publish(status: CaptureStatus) {
        mutableState.value = CaptureUiState(
            status = status,
            evidence = evidence.snapshot(nowMillis()),
        )
    }
}

/** Keeps user-facing capture failures within the fixed diagnostic/state bound. */
internal fun boundedCaptureErrorDetail(message: String): String {
    val suffix = "…"
    val suffixBytes = suffix.toByteArray(Charsets.UTF_8).size
    val messageBytes = message.toByteArray(Charsets.UTF_8).size
    if (messageBytes <= MAX_CAPTURE_ERROR_DETAIL_BYTES) {
        return message
    }

    val prefixLimit = MAX_CAPTURE_ERROR_DETAIL_BYTES - suffixBytes
    var prefixBytes = 0
    var index = 0
    while (index < message.length) {
        val codePoint = message.codePointAt(index)
        val codePointBytes = String(Character.toChars(codePoint)).toByteArray(Charsets.UTF_8).size
        if (prefixBytes + codePointBytes > prefixLimit) {
            break
        }
        prefixBytes += codePointBytes
        index += Character.charCount(codePoint)
    }
    return message.substring(0, index) + suffix
}
