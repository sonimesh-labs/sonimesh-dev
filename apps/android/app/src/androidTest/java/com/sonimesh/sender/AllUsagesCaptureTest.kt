package com.sonimesh.sender

import android.app.KeyguardManager
import android.app.Notification
import android.app.NotificationManager
import android.content.ClipboardManager
import android.content.Intent
import android.graphics.Rect
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
import com.sonimesh.sender.capture.isActive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.sin

class AllUsagesCaptureTest {
    private var lastScreenSnapshot = "no accessibility nodes"

    @Test
    fun built_in_tone_is_captured_and_stops_with_activity() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        try {
            ActivityScenario.launch(MainActivity::class.java).use { scenario ->
                assertTrue("start button missing: $lastScreenSnapshot", clickText(automation, "Start capture", scroll = true))
                assertTrue("projection chooser missing", clickText(automation, "Share one app"))
                assertTrue("entire screen option missing", clickText(automation, "Share entire screen"))
                assertTrue("projection confirmation missing", clickText(automation, "Share screen"))
                assertTrue("capture did not become silent", waitUntil(10_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.NoCapturableAudio
                })
                val before = nonSilentFrames()
                assertTrue(
                    "test tone control missing: $lastScreenSnapshot",
                    clickText(automation, "Start test tone", scroll = true),
                )
                val detected = waitUntil(8_000) {
                    nonSilentFrames() - before >= 2_000
                }
                assertTrue("built-in tone was not captured: ${CaptureStateStore.state.value}", detected)
                assertTrue("recent tone peak missing", waitUntil(8_000) {
                    (CaptureStateStore.state.value.evidence.recentPeakSampleMagnitude ?: 0) > 0
                })
                assertTrue(
                    "recent peak label missing: $lastScreenSnapshot",
                    findOnScreen(automation, scroll = true, initialScrollForward = false) {
                        it.containsText("Recent peak level")
                    },
                )
                assertTrue(
                    "tone stop control missing: $lastScreenSnapshot",
                    clickText(automation, "Stop test tone", scroll = true),
                )
                assertTrue("capture did not return to silence", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.NoCapturableAudio
                })
                assertTrue("recent peak did not fall to zero", waitUntil(8_000) {
                    CaptureStateStore.state.value.evidence.recentPeakSampleMagnitude == 0
                })
                assertTrue("recent zero level missing: $lastScreenSnapshot", findOnScreen(automation, scroll = true, initialScrollForward = false) {
                    it.containsText("0.000%")
                })
                assertTrue("run peak was lost after silence", (CaptureStateStore.state.value.evidence.peakSampleMagnitude ?: 0) > 0)
                val beforeRestart = nonSilentFrames()
                assertTrue(
                    "test tone could not restart: $lastScreenSnapshot",
                    clickText(automation, "Start test tone", scroll = true),
                )
                assertTrue("restarted tone was not captured", waitUntil(8_000) {
                    nonSilentFrames() - beforeRestart >= 2_000
                })
                scenario.recreate()
                assertTrue("activity recreation did not stop the tone", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.NoCapturableAudio
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
    fun reopening_activity_shows_active_capture_usage() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                assertTrue("usage selector missing: $lastScreenSnapshot", clickText(automation, "All supported", scroll = true))
                assertTrue("start button missing: $lastScreenSnapshot", clickText(automation, "Start capture", scroll = true))
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
                assertTrue("capture notification missing return action", notification?.contentIntent != null)
                automation.executeShellCommand("input keyevent KEYCODE_HOME").close()
                assertTrue("home screen did not appear", waitUntil(8_000) {
                    val name = automation.rootInActiveWindow?.packageName?.toString()
                    name != null && name != instrumentation.targetContext.packageName
                })
                notification?.contentIntent?.send()
                assertTrue("notification did not return to capture activity", waitUntil(8_000) {
                    automation.rootInActiveWindow?.packageName?.toString() == instrumentation.targetContext.packageName
                })
                assertTrue("notification return stopped the active capture", CaptureStateStore.state.value.status.isActive())
            }
            ActivityScenario.launch(MainActivity::class.java).use {
                val selected = findOnScreen(automation, scroll = true) { it.hasSelectedText("All supported") }
                assertTrue("reopened activity did not show the active usage", selected)
                assertTrue(
                    "stop button missing: $lastScreenSnapshot",
                    clickText(automation, "Stop capture", scroll = true),
                )
                assertTrue("capture did not stop", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.Stopped
                })
                assertTrue("usage was not retained after stop", findOnScreen(automation, scroll = true) {
                    it.hasSelectedText("All supported")
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
    fun notification_stop_action_ends_capture_from_background() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        try {
            ActivityScenario.launch(MainActivity::class.java).use {
                assertTrue("start button missing: $lastScreenSnapshot", clickText(automation, "Start capture", scroll = true))
                assertTrue("projection chooser missing", clickText(automation, "Share one app"))
                assertTrue("entire screen option missing", clickText(automation, "Share entire screen"))
                assertTrue("projection confirmation missing", clickText(automation, "Share screen"))
                assertTrue("capture did not start", waitUntil(10_000) {
                    CaptureStateStore.state.value.status.isActive()
                })
                val notificationManager = instrumentation.targetContext
                    .getSystemService(NotificationManager::class.java)
                val stopAction = notificationManager.activeNotifications
                    .firstOrNull { it.id == 1001 }
                    ?.notification
                    ?.actions
                    ?.singleOrNull()
                assertTrue("notification stop action missing", stopAction?.actionIntent != null)

                automation.executeShellCommand("input keyevent KEYCODE_HOME").close()
                assertTrue("home screen did not appear", waitUntil(8_000) {
                    val name = automation.rootInActiveWindow?.packageName?.toString()
                    name != null && name != instrumentation.targetContext.packageName
                })
                stopAction?.actionIntent?.send()
                assertTrue("notification action did not stop capture", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.Stopped
                })
                assertTrue("stopped capture notification remained visible", waitUntil(5_000) {
                    notificationManager.activeNotifications.none { it.id == 1001 }
                })
            }
        } finally {
            instrumentation.targetContext.startService(
                Intent(instrumentation.targetContext, CaptureService::class.java)
                    .setAction(CaptureService.ACTION_STOP),
            )
            waitUntil(5_000) { CaptureStateStore.state.value.status is CaptureStatus.Stopped }
        }
    }

    @Test
    fun capture_remains_active_while_screen_is_off() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        val context = instrumentation.targetContext
        val keyguardManager = context.getSystemService(KeyguardManager::class.java)
        val powerManager = context.getSystemService(android.os.PowerManager::class.java)
        val notificationManager = context.getSystemService(NotificationManager::class.java)
        try {
            automation.executeShellCommand("input keyevent KEYCODE_WAKEUP").close()
            automation.executeShellCommand("wm dismiss-keyguard").close()
            ActivityScenario.launch(MainActivity::class.java).use {
                assertTrue("start button missing: $lastScreenSnapshot", clickText(automation, "Start capture", scroll = true))
                assertTrue("projection chooser missing", clickText(automation, "Share one app"))
                assertTrue("entire screen option missing", clickText(automation, "Share entire screen"))
                assertTrue("projection confirmation missing", clickText(automation, "Share screen"))
                assertTrue("capture did not start", waitUntil(10_000) {
                    CaptureStateStore.state.value.status.isActive()
                })

                automation.executeShellCommand("input keyevent KEYCODE_POWER").close()
                assertTrue("screen did not enter keyguard", waitUntil(8_000) {
                    !powerManager.isInteractive && keyguardManager.isKeyguardLocked
                })
                SystemClock.sleep(2_000)
                assertTrue("capture stopped while the screen was off", CaptureStateStore.state.value.status.isActive())
                assertTrue(
                    "foreground notification disappeared while the screen was off",
                    notificationManager.activeNotifications.any { it.id == 1001 },
                )

                automation.executeShellCommand("input keyevent KEYCODE_POWER").close()
                assertTrue("screen did not wake", waitUntil(8_000) { powerManager.isInteractive })
                automation.executeShellCommand("wm dismiss-keyguard").close()
                assertTrue("capture did not survive wake", CaptureStateStore.state.value.status.isActive())
                assertTrue(
                    "stop button missing after wake: $lastScreenSnapshot",
                    clickText(automation, "Stop capture", scroll = true),
                )
                assertTrue("capture did not stop after wake", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.Stopped
                })
            }
        } finally {
            context.startService(
                Intent(context, CaptureService::class.java).setAction(CaptureService.ACTION_STOP),
            )
            waitUntil(5_000) { CaptureStateStore.state.value.status is CaptureStatus.Stopped }
        }
    }

    @Test
    fun captures_each_supported_usage_in_one_session() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            try {
                assertTrue("usage selector missing: $lastScreenSnapshot", clickText(automation, "All supported", scroll = true))
                // Start immediately, before waiting for the selection to redraw.
                assertTrue("start button missing: $lastScreenSnapshot", clickText(automation, "Start capture", scroll = true))
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
                    assertTrue("notification did not report the absence of capturable audio", waitUntil(8_000) {
                        captureNotificationText() == "No capturable audio detected"
                    })
                    val before = nonSilentFrames()
                    val track = tone(usage)
                    try {
                        track.play()
                        assertTrue("no captured signal for usage $usage", waitUntil(8_000) {
                            nonSilentFrames() - before >= 2_000
                        })
                        assertTrue("notification did not report the captured signal", waitUntil(8_000) {
                            captureNotificationText() == "Audio signal observed"
                        })
                    } finally {
                        track.stop()
                        track.release()
                    }
                }
                assertTrue(
                    "usage check control missing: $lastScreenSnapshot",
                    clickText(automation, "Check supported usages", scroll = true),
                )
                assertTrue("usage check did not report each captured test tone", waitUntil(15_000) {
                    automation.rootInActiveWindow?.containsText(
                        "Usage check: Media: signal observed; Game: signal observed; Other: signal observed",
                    ) == true
                })
                scenario.recreate()
                assertTrue("completed usage check was not restored", waitUntil(8_000) {
                    automation.rootInActiveWindow?.containsText(
                        "Usage check: Media: signal observed; Game: signal observed; Other: signal observed",
                    ) == true
                })
                assertTrue("copy evidence control missing", clickText(automation, "Copy capture evidence", scroll = true))
                assertTrue("completed usage check was not copied", clipboardText().contains("Usage check: Media: signal observed"))
                assertTrue(
                    "second usage check could not start",
                    clickText(automation, "Check supported usages", scroll = true),
                )
                assertTrue(
                    "usage check cancellation control missing",
                    clickText(automation, "Cancel usage check", scroll = true),
                )
                assertTrue("usage check did not report cancellation", waitUntil(5_000) {
                    automation.rootInActiveWindow?.containsText("Usage check stopped") == true
                })
                scenario.recreate()
                assertTrue("copy evidence control missing after cancellation", clickText(automation, "Copy capture evidence", scroll = true))
                assertTrue("canceled check retained a previous result", !clipboardText().contains("Usage check:"))
                assertTrue("cancellation did not stop the usage tone", waitUntil(8_000) {
                    CaptureStateStore.state.value.status is CaptureStatus.NoCapturableAudio
                })
                assertTrue("usage check could not restart after cancellation", clickText(automation, "Check supported usages", scroll = true))
                assertTrue("usage check did not finish after cancellation", waitUntil(15_000) {
                    automation.rootInActiveWindow?.containsText(
                        "Usage check: Media: signal observed; Game: signal observed; Other: signal observed",
                    ) == true
                })
                val backgroundTone = tone(AudioAttributes.USAGE_MEDIA)
                try {
                    backgroundTone.play()
                    assertTrue("background tone was not captured", waitUntil(8_000) {
                        CaptureStateStore.state.value.status is CaptureStatus.Capturing
                    })
                    assertTrue("usage check could not start with background audio", clickText(automation, "Check supported usages", scroll = true))
                    assertTrue("background audio did not make the check inconclusive", waitUntil(10_000) {
                        automation.rootInActiveWindow?.containsText("Usage check inconclusive. Pause other audio, check capture, and try again.") == true
                    })
                    assertTrue("copy evidence control missing after inconclusive check", clickText(automation, "Copy capture evidence", scroll = true))
                    assertTrue("inconclusive check retained a previous result", !clipboardText().contains("Usage check:"))
                } finally {
                    backgroundTone.stop()
                    backgroundTone.release()
                }
                assertTrue("usage check could not retry", clickText(automation, "Check supported usages", scroll = true))
                assertTrue("usage check did not recover after background audio stopped", waitUntil(15_000) {
                    automation.rootInActiveWindow?.containsText(
                        "Usage check: Media: signal observed; Game: signal observed; Other: signal observed",
                    ) == true
                })
                assertTrue("copy evidence control missing after retry", clickText(automation, "Copy capture evidence", scroll = true))
                assertTrue("completed retry was not copied", clipboardText().contains("Usage check: Media: signal observed"))
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

    private fun clipboardText(): String {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val clip = requireNotNull(context.getSystemService(ClipboardManager::class.java).primaryClip)
        return clip.getItemAt(0).coerceToText(context).toString()
    }

    private fun captureNotificationText(): String? =
        InstrumentationRegistry.getInstrumentation().targetContext.getSystemService(NotificationManager::class.java)
            .activeNotifications.firstOrNull { it.id == 1001 }?.notification?.extras
            ?.getCharSequence(Notification.EXTRA_TEXT)?.toString()

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

    private fun clickText(automation: android.app.UiAutomation, text: String, scroll: Boolean = false): Boolean =
        findOnScreen(
            automation,
            scroll,
            initialScrollForward = !text.startsWith("Stop ") && !text.startsWith("Cancel "),
        ) {
            it.clickMatchingText(text)
        }

    private fun findOnScreen(
        automation: android.app.UiAutomation,
        scroll: Boolean,
        initialScrollForward: Boolean = true,
        match: (AccessibilityNodeInfo) -> Boolean,
    ): Boolean {
        var scrollForward = initialScrollForward
        return waitUntil(8_000) {
            val root = automation.rootInActiveWindow ?: return@waitUntil false
            lastScreenSnapshot = root.textSnapshot()
            if (match(root)) return@waitUntil true
            if (scroll) {
                val action = if (scrollForward) AccessibilityNodeInfo.ACTION_SCROLL_FORWARD else AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
                val before = lastScreenSnapshot
                val result = root.findScrollable()?.performAction(action)
                SystemClock.sleep(500)
                val updatedRoot = automation.rootInActiveWindow
                val after = updatedRoot?.textSnapshot()
                if (result != true || after == before) scrollForward = !scrollForward
            }
            false
        }
    }

    private fun AccessibilityNodeInfo.textSnapshot(): String {
        val entries = mutableListOf<String>()
        fun collect(node: AccessibilityNodeInfo) {
            node.text?.toString()?.let {
                val bounds = Rect()
                node.getBoundsInScreen(bounds)
                entries += "$it@${bounds.toShortString()}"
            }
            for (index in 0 until node.childCount) {
                node.getChild(index)?.let(::collect)
            }
        }
        collect(this)
        return entries.joinToString(limit = 24, truncated = "...")
            .ifEmpty { "no text nodes" }
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

    private fun AccessibilityNodeInfo.containsText(text: String): Boolean {
        if (this.text?.toString() == text) return true
        for (index in 0 until childCount) {
            if (getChild(index)?.containsText(text) == true) return true
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
