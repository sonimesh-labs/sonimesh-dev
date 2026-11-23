package com.sonimesh.opusspike

import android.app.ActivityManager
import android.os.Bundle
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

class PairedCaptureTest {
    @Test
    fun captures_tone_after_pairing_and_stops() = runCapture(expectDisconnect = false)

    @Test
    fun captures_again_after_stopping() {
        runCapture(expectDisconnect = false)
        runCapture(expectDisconnect = false)
    }

    @Test
    fun stops_capture_when_desktop_disconnects() = runCapture(expectDisconnect = true)

    private fun runCapture(expectDisconnect: Boolean) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val args = InstrumentationRegistry.getArguments()
        assumeTrue(
            "paired audio lab requires pairingInvitationHex or pairingSpkiHex",
            args.containsKey("pairingInvitationHex") || args.containsKey("pairingSpkiHex"),
        )
        val pin = args.getString("pairingSpkiHex")
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                args.getString("mediaHost")?.let { mediaHost ->
                    (find(root) { it is EditText && it.contentDescription == "Desktop IPv4 address for UDP" } as EditText)
                        .setText(mediaHost)
                }
                args.getString("pairingInvitationHex")?.let { payload ->
                    (find(root) { it is EditText && it.contentDescription == "Lab invitation payload" } as EditText)
                        .setText(payload)
                    find(root) { it is Button && it.text.toString().equals("Apply invitation payload", true) }.performClick()
                } ?: run {
                    (find(root) { it is EditText && it.contentDescription == "Paired lab desktop SPKI" } as EditText)
                        .setText(requireNotNull(pin))
                    args.getString("pairingHost")?.let { host ->
                        (find(root) { it is EditText && it.contentDescription == "Paired control IPv4 address" } as EditText).setText(host)
                    }
                }
                find(root) { it is Button && it.text.toString().equals("Toggle test tone", true) }.performClick()
                find(root) { it is Button && it.text.toString().equals("Start paired UDP capture", true) }.performClick()
            }
            val automation = instrumentation.uiAutomation
            assertTrue("screen-capture selector missing", clickNode(automation, "Share one app"))
            assertTrue("entire-screen option missing", clickNode(automation, "Share entire screen"))
            assertTrue("screen-capture consent missing", clickNode(automation, "Share screen"))
            var capturing = false
            val deadline = SystemClock.elapsedRealtime() + 20_000
            while (!capturing && SystemClock.elapsedRealtime() < deadline) {
                scenario.onActivity { activity ->
                    capturing = containsStatus(activity.window.decorView, "Status: Capturing audio")
                }
                if (!capturing) SystemClock.sleep(100)
            }
            assertTrue("capture did not start", capturing)
            scenario.onActivity { activity ->
                find(activity.window.decorView) { it is Button && it.text.toString().equals("Start paired UDP capture", true) }.performClick()
                assertTrue(
                    "duplicate capture request replaced the active capture status",
                    containsStatus(activity.window.decorView, "Status: Capturing audio"),
                )
            }
            if (expectDisconnect) {
                instrumentation.sendStatus(1, Bundle().apply { putString("pairedCapture", "ready") })
                var disconnected = false
                val disconnectDeadline = SystemClock.elapsedRealtime() + 20_000
                while (!disconnected && SystemClock.elapsedRealtime() < disconnectDeadline) {
                    scenario.onActivity { activity ->
                        disconnected = containsStatus(activity.window.decorView, "Status: Desktop connection ended.")
                    }
                    if (!disconnected) SystemClock.sleep(100)
                }
                assertTrue("desktop disconnect was not reported", disconnected)
            } else {
                SystemClock.sleep(3_000)
                scenario.onActivity { activity ->
                    val stop = find(activity.window.decorView) { it is Button && it.text.toString().equals("Stop capture", true) }
                    stop.performClick()
                    stop.performClick()
                }
            }
            var stopped = false
            val stopDeadline = SystemClock.elapsedRealtime() + 10_000
            while (!stopped && SystemClock.elapsedRealtime() < stopDeadline) {
                scenario.onActivity { activity ->
                    @Suppress("DEPRECATION")
                    stopped = activity.getSystemService(ActivityManager::class.java).getRunningServices(Int.MAX_VALUE)
                        .none { it.service.className == LabCaptureService::class.java.name }
                }
                if (!stopped) SystemClock.sleep(100)
            }
            assertTrue("capture service did not stop", stopped)
            if (!expectDisconnect) {
                scenario.onActivity { activity ->
                    assertTrue("capture did not publish completion", containsStatus(activity.window.decorView, "Status: Capture stopped."))
                }
            }
        }
    }

    private fun clickNode(automation: android.app.UiAutomation, text: String): Boolean {
        val deadline = SystemClock.elapsedRealtime() + 40_000
        while (SystemClock.elapsedRealtime() < deadline) {
            val root = automation.rootInActiveWindow
            val node = root?.findAccessibilityNodeInfosByText(text)?.firstOrNull()
            if (node != null) {
                var target: AccessibilityNodeInfo? = node
                while (target != null && !target.isClickable) target = target.parent
                if (target?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true) return true
            }
            SystemClock.sleep(100)
        }
        return false
    }

    private fun find(view: View, predicate: (View) -> Boolean): View {
        if (predicate(view)) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            val found = runCatching { find(view.getChildAt(index), predicate) }.getOrNull()
            if (found != null) return found
        }
        error("view not found")
    }

    private fun containsStatus(view: View, prefix: String): Boolean =
        runCatching { find(view) { it is TextView && it.text.toString().startsWith(prefix) } }.isSuccess
}
