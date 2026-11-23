package com.sonimesh.opusspike

import android.app.Activity
import android.app.ActivityManager
import android.app.NotificationManager
import android.content.Intent
import android.os.Bundle
import android.os.PowerManager
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Button
import android.widget.EditText
import android.widget.Spinner
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

internal fun deliverScannerResult(activity: LabActivity, requestCode: Int, resultCode: Int, contents: String?) {
    val callback = LabActivity::class.java.getDeclaredMethod(
        "onActivityResult", Int::class.javaPrimitiveType, Int::class.javaPrimitiveType, Intent::class.java,
    )
    callback.isAccessible = true
    callback.invoke(activity, requestCode, resultCode, Intent().putExtra("SCAN_RESULT", contents))
}

class PairedCaptureTest {
    @Test
    fun captures_tone_after_pairing_and_stops() = runCapture(expectDisconnect = false)

    @Test
    fun captures_after_scanned_invitation() = runCapture(expectDisconnect = false, startFromScan = true)

    @Test
    fun captures_again_after_stopping() {
        runCapture(expectDisconnect = false)
        runCapture(expectDisconnect = false)
    }

    @Test
    fun keeps_service_active_while_screen_is_off() = runCapture(expectDisconnect = false, screenOff = true)

    @Test
    fun stops_capture_when_desktop_disconnects() = runCapture(expectDisconnect = true)

    @Test
    fun returns_to_active_capture_from_notification() = runCapture(expectDisconnect = false, notificationReturn = true)

    private fun runCapture(expectDisconnect: Boolean, screenOff: Boolean = false, notificationReturn: Boolean = false, startFromScan: Boolean = false) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val args = InstrumentationRegistry.getArguments()
        assumeTrue(
            "paired audio lab requires pairingInvitationHex or pairingSpkiHex",
            args.containsKey("pairingInvitationHex") || args.containsKey("pairingSpkiHex"),
        )
        val pin = args.getString("pairingSpkiHex")
        if (startFromScan) assumeTrue("scan test requires pairingInvitationHex", args.containsKey("pairingInvitationHex"))
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            args.getString("bitrateKbps")?.let { requested ->
                val position = listOf("64", "96", "128").indexOf(requested)
                require(position >= 0) { "unsupported test bitrate" }
                scenario.onActivity { activity ->
                    val spinner = find(activity.window.decorView) {
                        it is Spinner && it.contentDescription == "Opus target bitrate for capture"
                    } as Spinner
                    spinner.setSelection(position)
                }
                instrumentation.waitForIdleSync()
            }
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                args.getString("bitrateKbps")?.let { requested ->
                    val spinner = find(root) {
                        it is Spinner && it.contentDescription == "Opus target bitrate for capture"
                    } as Spinner
                    assertEquals("$requested kbps", spinner.selectedItem)
                }
                args.getString("mediaHost")?.let { mediaHost ->
                    (find(root) { it is EditText && it.contentDescription == "Desktop IPv4 address for UDP" } as EditText)
                        .setText(mediaHost)
                }
                find(root) { it is Button && it.text.toString().equals("Start test tone", true) }.performClick()
                if (startFromScan) {
                    deliverScannerResult(activity, 4, Activity.RESULT_OK, args.getString("pairingInvitationHex"))
                } else {
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
                    find(root) { it is Button && it.text.toString().equals("Start paired UDP capture", true) }.performClick()
                }
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
                find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Sender queue drops: [0-9]+"))
                }
                find(activity.window.decorView) { it is Button && it.text.toString().equals("Start paired UDP capture", true) }.performClick()
                assertTrue(
                    "duplicate capture request replaced the active capture status",
                    containsStatus(activity.window.decorView, "Status: Capturing audio"),
                )
            }
            var disconnectStatus: String? = null
            if (expectDisconnect) {
                instrumentation.sendStatus(1, Bundle().apply { putString("pairedCapture", "ready") })
                val disconnectDeadline = SystemClock.elapsedRealtime() + 20_000
                while (disconnectStatus == null && SystemClock.elapsedRealtime() < disconnectDeadline) {
                    scenario.onActivity { activity ->
                        val root = activity.window.decorView
                        disconnectStatus = listOf(
                            "Status: Desktop connection ended. Check the listener, then start capture again.",
                            "Status: UDP sending failed. Check the network and listener, then start capture again.",
                        ).firstOrNull { containsStatus(root, it) }
                    }
                    if (disconnectStatus == null) SystemClock.sleep(100)
                }
                assertTrue("desktop disconnect was not reported", disconnectStatus != null)
            } else {
                if (notificationReturn) {
                    automation.executeShellCommand("input keyevent KEYCODE_HOME").close()
                    val homeDeadline = SystemClock.elapsedRealtime() + 10_000
                    var homeVisible = false
                    while (!homeVisible && SystemClock.elapsedRealtime() < homeDeadline) {
                        val name = automation.rootInActiveWindow?.packageName?.toString()
                        homeVisible = name != null && name != instrumentation.targetContext.packageName
                        if (!homeVisible) SystemClock.sleep(100)
                    }
                    assertTrue("home screen did not appear", homeVisible)
                    val notification = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                        .activeNotifications.firstOrNull { it.id == 1 }?.notification
                    requireNotNull(notification?.contentIntent) { "capture notification has no open action" }.send()
                    val returnDeadline = SystemClock.elapsedRealtime() + 10_000
                    var returned = false
                    while (!returned && SystemClock.elapsedRealtime() < returnDeadline) {
                        val root = automation.rootInActiveWindow
                        returned = root?.packageName?.toString() == instrumentation.targetContext.packageName &&
                            root.findAccessibilityNodeInfosByText("Status: Capturing audio").isNotEmpty()
                        if (!returned) SystemClock.sleep(100)
                    }
                    assertTrue("notification did not return to active capture", returned)
                    scenario.onActivity { activity ->
                        assertTrue("returning from notification lost capture status",
                            containsStatus(activity.window.decorView, "Status: Capturing audio"))
                    }
                    SystemClock.sleep(3_000)
                } else if (screenOff) {
                    val power = instrumentation.targetContext.getSystemService(PowerManager::class.java)
                    try {
                        automation.executeShellCommand("input keyevent KEYCODE_SLEEP").close()
                        val sleepDeadline = SystemClock.elapsedRealtime() + 5_000
                        while (power.isInteractive && SystemClock.elapsedRealtime() < sleepDeadline) SystemClock.sleep(100)
                        assertTrue("screen did not turn off", !power.isInteractive)
                        instrumentation.sendStatus(1, Bundle().apply { putString("pairedCapture", "screen-off") })
                        SystemClock.sleep(8_000)
                        assertTrue("screen became interactive during capture", !power.isInteractive)
                        @Suppress("DEPRECATION")
                        val services = instrumentation.targetContext.getSystemService(ActivityManager::class.java)
                            .getRunningServices(Int.MAX_VALUE)
                        assertTrue("capture service stopped with the screen off", services.any {
                            it.service.className == LabCaptureService::class.java.name
                        })
                    } finally {
                        automation.executeShellCommand("input keyevent KEYCODE_WAKEUP").close()
                    }
                } else {
                    SystemClock.sleep(3_000)
                }
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
            scenario.onActivity { activity ->
                val count = find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Sender queue drops: [0-9]+"))
                }
                assertTrue("final sender queue count was not retained", count is TextView)
            }
            if (expectDisconnect) {
                scenario.onActivity { activity ->
                    assertTrue("disconnect reason was not retained", containsStatus(activity.window.decorView, requireNotNull(disconnectStatus)))
                }
            } else {
                scenario.onActivity { activity ->
                    assertTrue("capture did not publish completion", containsStatus(activity.window.decorView, "Status: Capture stopped."))
                    val rate = find(activity.window.decorView) {
                        it is TextView && it.text.toString().matches(Regex("Encoded payload average: [0-9]+ kbps"))
                    } as TextView
                    instrumentation.sendStatus(1, Bundle().apply { putString("encodedPayload", rate.text.toString()) })
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
