package com.sonimesh.sender

import android.app.Notification
import android.app.NotificationManager
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import com.sonimesh.sender.capture.CaptureService
import com.sonimesh.sender.capture.CaptureSource
import com.sonimesh.sender.capture.CaptureStateStore
import com.sonimesh.sender.capture.CaptureStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.sin

class AllUsagesCaptureTest {
    @Test
    fun reopening_activity_shows_active_capture_usage() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                assertTrue("usage selector missing", clickText(automation, "All supported", scroll = true))
                assertTrue("start button missing", clickText(automation, "Start capture", scroll = true))
                assertTrue("projection chooser missing", clickText(automation, "Share one app"))
                assertTrue("entire screen option missing", clickText(automation, "Share entire screen"))
                assertTrue("projection confirmation missing", clickText(automation, "Share screen"))
                assertTrue("capture did not start", waitUntil(10_000) {
                    CaptureStateStore.state.value.evidence.captureSource == CaptureSource.AllSupported
                })
                val notification = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                    .activeNotifications.firstOrNull { it.id == 1001 }?.notification
                assertTrue("capture notification missing", notification != null)
                assertTrue(
                    "capture notification did not expose duration",
                    notification?.extras?.getString(Notification.EXTRA_SUB_TEXT)?.startsWith("Capture duration: ") == true,
                )
            }
            ActivityScenario.launch(MainActivity::class.java).use {
                val selected = waitUntil(8_000) {
                    val root = automation.rootInActiveWindow ?: return@waitUntil false
                    if (root.hasSelectedText("All supported")) return@waitUntil true
                    root.findScrollable()?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
                    false
                }
                assertTrue("reopened activity did not show the active usage", selected)
                assertTrue("stop button missing", clickText(automation, "Stop capture", scroll = true))
                assertTrue("capture did not stop", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.Stopped
                })
                assertTrue("usage was not retained after stop", waitUntil(8_000) {
                    automation.rootInActiveWindow?.hasSelectedText("All supported") == true
                })
            }
        } finally {
            instrumentation.targetContext.startService(
                Intent(instrumentation.targetContext, CaptureService::class.java).setAction(CaptureService.ACTION_STOP),
            )
            waitUntil(5_000) { CaptureStateStore.state.value.status is CaptureStatus.Stopped }
        }
    }

    @Test
    fun captures_each_supported_usage_in_one_session() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        ActivityScenario.launch(MainActivity::class.java).use {
            try {
                assertTrue("usage selector missing", clickText(automation, "All supported", scroll = true))
                // Start immediately, before waiting for the selection to redraw.
                assertTrue("start button missing", clickText(automation, "Start capture", scroll = true))
                assertTrue("projection chooser missing", clickText(automation, "Share one app"))
                assertTrue("entire screen option missing", clickText(automation, "Share entire screen"))
                assertTrue("projection confirmation missing", clickText(automation, "Share screen"))
                val started = waitUntil(10_000) {
                    CaptureStateStore.state.value.evidence.captureSource == CaptureSource.AllSupported
                }
                assertTrue("capture did not start: ${CaptureStateStore.state.value}", started)

                for (usage in listOf(AudioAttributes.USAGE_MEDIA, AudioAttributes.USAGE_GAME, AudioAttributes.USAGE_UNKNOWN)) {
                    assertTrue("capture did not return to silence before usage $usage", waitUntil(8_000) {
                        CaptureStateStore.state.value.status is CaptureStatus.NoCapturableAudio
                    })
                    val before = nonSilentFrames()
                    val track = tone(usage)
                    try {
                        track.play()
                        assertTrue("no captured signal for usage $usage", waitUntil(8_000) {
                            nonSilentFrames() - before >= 2_000
                        })
                    } finally {
                        track.stop()
                        track.release()
                    }
                }
                assertEquals(0L, CaptureStateStore.state.value.evidence.readErrors)
            } finally {
                instrumentation.targetContext.startService(
                    Intent(instrumentation.targetContext, CaptureService::class.java)
                        .setAction(CaptureService.ACTION_STOP),
                )
                waitUntil(5_000) { CaptureStateStore.state.value.status is CaptureStatus.Stopped }
            }
        }
    }

    private fun nonSilentFrames(): Long {
        val evidence = CaptureStateStore.state.value.evidence
        return evidence.capturedFrames - evidence.silentFrames
    }

    private fun tone(usage: Int): AudioTrack {
        val samples = ShortArray(4_800) { index -> (sin(2.0 * PI * 440 * index / 48_000) * 12_000).toInt().toShort() }
        val track = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(usage)
                .setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_ALL).build())
            .setAudioFormat(AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(48_000).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setTransferMode(AudioTrack.MODE_STATIC)
            .setBufferSizeInBytes(samples.size * Short.SIZE_BYTES)
            .build()
        check(track.write(samples, 0, samples.size) == samples.size)
        check(track.setLoopPoints(0, samples.size, -1) == AudioTrack.SUCCESS)
        return track
    }

    private fun waitUntil(timeoutMs: Long, condition: () -> Boolean): Boolean {
        val deadline = SystemClock.elapsedRealtime() + timeoutMs
        while (SystemClock.elapsedRealtime() < deadline) {
            if (condition()) return true
            SystemClock.sleep(100)
        }
        return condition()
    }

    private fun clickText(automation: android.app.UiAutomation, text: String, scroll: Boolean = false): Boolean {
        return waitUntil(8_000) {
            val root = automation.rootInActiveWindow ?: return@waitUntil false
            if (root.clickMatchingText(text)) return@waitUntil true
            if (scroll) root.findScrollable()?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
            false
        }
    }

    private fun AccessibilityNodeInfo.clickMatchingText(text: String): Boolean {
        if (this.text?.toString() == text) {
            var current: AccessibilityNodeInfo? = this
            while (current != null) {
                if (current.isClickable && current.isEnabled && current.performAction(AccessibilityNodeInfo.ACTION_CLICK)) return true
                current = current.parent
            }
        }
        for (index in 0 until childCount) {
            if (getChild(index)?.clickMatchingText(text) == true) return true
        }
        return false
    }

    private fun AccessibilityNodeInfo.hasSelectedText(text: String): Boolean {
        if (this.text?.toString() == text) {
            var current: AccessibilityNodeInfo? = this
            while (current != null) {
                if (current.isSelected || current.isChecked) return true
                current = current.parent
            }
        }
        for (index in 0 until childCount) {
            if (getChild(index)?.hasSelectedText(text) == true) return true
        }
        return false
    }

    private fun AccessibilityNodeInfo.findScrollable(): AccessibilityNodeInfo? {
        if (isScrollable) return this
        for (index in 0 until childCount) {
            val found = getChild(index)?.findScrollable()
            if (found != null) return found
        }
        return null
    }
}
