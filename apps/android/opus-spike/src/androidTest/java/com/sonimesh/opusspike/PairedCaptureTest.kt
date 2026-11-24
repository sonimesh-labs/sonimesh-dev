package com.sonimesh.opusspike

import android.app.Activity
import android.app.ActivityManager
import android.app.Notification
import android.app.NotificationManager
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.os.Bundle
import android.os.PowerManager
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.Button
import android.widget.EditText
import android.widget.Spinner
import android.widget.Switch
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
    fun notificationDenialDoesNotStartCapture() {
        assumeTrue("notification permission is available on Android 13+", android.os.Build.VERSION.SDK_INT >= 33)
        val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
        val packageName = InstrumentationRegistry.getInstrumentation().targetContext.packageName
        automation.executeShellCommand("pm grant $packageName android.permission.RECORD_AUDIO").close()
        automation.executeShellCommand("pm revoke $packageName android.permission.POST_NOTIFICATIONS").close()
        automation.executeShellCommand("pm clear-permission-flags $packageName android.permission.POST_NOTIFICATIONS user-set user-fixed").close()
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    find(activity.window.decorView) { it is Button && it.text.toString() == "Start ADB capture" }.performClick()
                }
                assertTrue("notification permission dialog missing", clickNodeById(automation, "com.android.permissioncontroller:id/permission_deny_button"))
                var denied = false
                val deadline = SystemClock.elapsedRealtime() + 5_000
                while (!denied && SystemClock.elapsedRealtime() < deadline) {
                    scenario.onActivity { activity ->
                        denied = containsStatus(activity.window.decorView, "Status: Notification permission is required")
                        if (denied) assertTrue("capture remained busy after denial", LabStatus.captureAdmission.canBegin())
                    }
                    if (!denied) SystemClock.sleep(50)
                }
                assertTrue("notification denial not reported", denied)
                scenario.onActivity { activity ->
                    find(activity.window.decorView) { it is Button && it.text.toString() == "Start ADB capture" }.performClick()
                }
                assertTrue("notification permission retry missing", clickNodeById(automation, "com.android.permissioncontroller:id/permission_allow_button"))
                assertTrue("screen-capture consent missing after notification grant", clickNode(automation, "Share one app"))
                automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
                automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            }
        } finally {
            automation.executeShellCommand("pm grant $packageName android.permission.POST_NOTIFICATIONS").close()
        }
    }

    @Test
    fun captures_tone_after_pairing_and_stops() = runCapture(expectDisconnect = false)

    @Test
    fun captures_after_scanned_invitation() = runCapture(expectDisconnect = false, startFromScan = true)

    @Test
    fun captures_after_pasted_invitation() = runCapture(expectDisconnect = false, startFromPaste = true)

    @Test
    fun keeps_capturing_tone_after_activity_recreation() = runCapture(expectDisconnect = false, recreate = true)

    @Test
    fun tone_follows_active_capture_usage() = runCapture(expectDisconnect = false, changeUsage = true)

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

    @Test
    fun mutes_and_resumes_sending_during_capture() = runCapture(expectDisconnect = false, testMute = true)

    private fun runCapture(expectDisconnect: Boolean, screenOff: Boolean = false, notificationReturn: Boolean = false, startFromScan: Boolean = false, startFromPaste: Boolean = false, recreate: Boolean = false, changeUsage: Boolean = false, testMute: Boolean = false) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val args = InstrumentationRegistry.getArguments()
        assumeTrue(
            "paired audio lab requires pairingInvitationHex or pairingSpkiHex",
            args.containsKey("pairingInvitationHex") || args.containsKey("pairingSpkiHex"),
        )
        val pin = args.getString("pairingSpkiHex")
        if (startFromScan) assumeTrue("scan test requires pairingInvitationHex", args.containsKey("pairingInvitationHex"))
        if (startFromPaste) assumeTrue("paste test requires pairingInvitationHex", args.containsKey("pairingInvitationHex"))
        ActivityScenario.launch<LabActivity>(Intent(instrumentation.targetContext, LabActivity::class.java)).use { scenario ->
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
                if (changeUsage) {
                    (find(root) { it is Spinner && it.contentDescription == "Playback usage for capture" } as Spinner)
                        .setSelection(1)
                }
                find(root) { it is Button && it.text.toString().equals("Start test tone", true) }.performClick()
                if (startFromScan) {
                    deliverScannerResult(activity, 4, Activity.RESULT_OK, args.getString("pairingInvitationHex"))
                } else if (startFromPaste) {
                    (find(root) { it is EditText && it.contentDescription == "Lab invitation payload" } as EditText)
                        .setText(requireNotNull(args.getString("pairingInvitationHex")))
                    find(root) { it is Button && it.text.toString() == "Apply invitation and start paired capture" }.performClick()
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
            if (android.os.Build.VERSION.SDK_INT >= 33 &&
                instrumentation.targetContext.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                assertTrue("notification permission dialog missing", clickNodeById(automation, "com.android.permissioncontroller:id/permission_allow_button"))
            }
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
                val usage = if (changeUsage) "Media" else "All supported usages"
                val bitrate = args.getString("bitrateKbps") ?: "96"
                assertTrue(containsStatus(activity.window.decorView, "Session request: $usage, $bitrate kbps target"))
                find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Sender queue drops: [0-9]+"))
                }
                find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                }
                find(activity.window.decorView) { it is Button && it.text.toString().equals("Start paired UDP capture", true) }.performClick()
                assertTrue(
                    "duplicate capture request replaced the active capture status",
                    containsStatus(activity.window.decorView, "Status: Capturing audio"),
                )
                if (startFromPaste) {
                    val invitation = find(activity.window.decorView) { it is EditText && it.contentDescription == "Lab invitation payload" } as EditText
                    invitation.setText("draft")
                    find(activity.window.decorView) { it is Button && it.text.toString() == "Apply invitation and start paired capture" }.performClick()
                    assertEquals("draft", invitation.text.toString())
                    assertTrue(containsStatus(activity.window.decorView, "Status: Capturing audio"))
                }
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
                if (testMute) {
                    // Leave complete receiver observation windows on each side of the switch.
                    SystemClock.sleep(3_000)
                    val manager = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                    fun notificationAction(title: String): Notification.Action? =
                        manager.activeNotifications.firstOrNull { it.id == 1 }?.notification?.actions
                            ?.firstOrNull { it.title.toString() == title }
                    fun awaitAction(title: String): Notification.Action {
                        val deadline = SystemClock.elapsedRealtime() + 5_000
                        while (SystemClock.elapsedRealtime() < deadline) {
                            notificationAction(title)?.let { return it }
                            SystemClock.sleep(50)
                        }
                        error("notification action missing: $title")
                    }
                    var packetsBeforeMute = 0L
                    scenario.onActivity { activity ->
                        val count = find(activity.window.decorView) {
                            it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                        } as TextView
                        packetsBeforeMute = count.text.toString().substringAfter(": ").toLong()
                        val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                        assertTrue("mute control is unavailable", control.isEnabled && !control.isChecked)
                    }
                    automation.executeShellCommand("input keyevent KEYCODE_HOME").close()
                    awaitAction("Mute sending").actionIntent.send()
                    awaitAction("Unmute sending")
                    SystemClock.sleep(2_000)
                    requireNotNull(manager.activeNotifications.firstOrNull { it.id == 1 }?.notification?.contentIntent).send()
                    val returnDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (automation.rootInActiveWindow?.packageName?.toString() != instrumentation.targetContext.packageName &&
                        SystemClock.elapsedRealtime() < returnDeadline) {
                        SystemClock.sleep(50)
                    }
                    assertEquals("capture activity did not return", instrumentation.targetContext.packageName,
                        automation.rootInActiveWindow?.packageName?.toString())
                    scenario.onActivity { activity ->
                        val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                        assertTrue("notification mute did not update activity", control.isChecked)
                    }
                    var sentWhileMuted = false
                    val muteDeadline = SystemClock.elapsedRealtime() + 5_000
                    while (!sentWhileMuted && SystemClock.elapsedRealtime() < muteDeadline) {
                        scenario.onActivity { activity ->
                            val count = find(activity.window.decorView) {
                                it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                            } as TextView
                            sentWhileMuted = count.text.toString().substringAfter(": ").toLong() > packetsBeforeMute
                        }
                        if (!sentWhileMuted) SystemClock.sleep(100)
                    }
                    assertTrue("media packets did not continue while muted", sentWhileMuted)
                    SystemClock.sleep(4_000)
                    scenario.recreate()
                    scenario.onActivity { activity ->
                        val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                        assertTrue("mute state did not survive recreation", control.isEnabled && control.isChecked)
                        assertTrue("capture stopped when muted", containsStatus(activity.window.decorView, "Status: Capturing audio"))
                        control.performClick()
                    }
                    awaitAction("Mute sending")
                    SystemClock.sleep(3_000)
                    scenario.onActivity { activity ->
                        val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                        assertTrue("unmute did not update control", control.isEnabled && !control.isChecked)
                        control.performClick()
                    }
                    awaitAction("Unmute sending").actionIntent.send()
                    awaitAction("Mute sending")
                    scenario.onActivity { activity ->
                        val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                        assertTrue("notification unmute did not update activity", !control.isChecked)
                    }
                } else if (changeUsage) {
                    scenario.onActivity { activity ->
                        val root = activity.window.decorView
                        (find(root) { it is Spinner && it.contentDescription == "Playback usage for capture" } as Spinner)
                            .setSelection(2)
                        assertTrue(containsStatus(root, "Session request: Media, ${args.getString("bitrateKbps") ?: "96"} kbps target"))
                        find(root) { it is Button && it.text.toString() == "Stop test tone" }.performClick()
                    }
                    var silent = false
                    val silentDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (!silent && SystemClock.elapsedRealtime() < silentDeadline) {
                        scenario.onActivity { activity ->
                            silent = containsStatus(activity.window.decorView, "Status: No capturable audio detected")
                        }
                        if (!silent) SystemClock.sleep(100)
                    }
                    assertTrue("capture did not detect silence after stopping the tone", silent)
                    scenario.recreate()
                    scenario.onActivity { activity ->
                        val root = activity.window.decorView
                        assertEquals(2, (find(root) { it is Spinner && it.contentDescription == "Playback usage for capture" } as Spinner).selectedItemPosition)
                        assertTrue(containsStatus(root, "Session request: Media, ${args.getString("bitrateKbps") ?: "96"} kbps target"))
                        find(root) { it is Button && it.text.toString() == "Start test tone" }.performClick()
                        assertEquals(AudioAttributes.USAGE_MEDIA, activity.testToneUsage())
                    }
                    var signal = false
                    val signalDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (!signal && SystemClock.elapsedRealtime() < signalDeadline) {
                        scenario.onActivity { activity ->
                            signal = containsStatus(activity.window.decorView, "Status: Capturing audio")
                        }
                        if (!signal) SystemClock.sleep(100)
                    }
                    assertTrue("capture did not detect the restarted tone", signal)
                    SystemClock.sleep(3_000)
                } else if (recreate) {
                    SystemClock.sleep(3_000)
                    scenario.recreate()
                    SystemClock.sleep(3_000)
                    scenario.onActivity { activity ->
                        assertTrue("test tone stopped after recreation", activity.isTestTonePlaying())
                        assertTrue("capture did not retain signal after recreation",
                            containsStatus(activity.window.decorView, "Status: Capturing audio"))
                    }
                } else if (notificationReturn) {
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
                    assertEquals("Transport: paired UDP", notification?.extras?.getString(Notification.EXTRA_TEXT))
                    val manager = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                    fun duration(): Long? = manager.activeNotifications.firstOrNull { it.id == 1 }
                        ?.notification?.extras?.getString(Notification.EXTRA_SUB_TEXT)
                        ?.let { Regex("Capture duration: ([0-9]+)s").matchEntire(it) }
                        ?.groupValues?.get(1)?.toLongOrNull()
                    val durationDeadline = SystemClock.elapsedRealtime() + 5_000
                    var initialDuration = duration()
                    while (initialDuration == null && SystemClock.elapsedRealtime() < durationDeadline) {
                        SystemClock.sleep(100)
                        initialDuration = duration()
                    }
                    assertTrue("capture notification did not expose a numeric duration", initialDuration != null)
                    val progressionDeadline = SystemClock.elapsedRealtime() + 5_000
                    var currentDuration = duration()
                    while ((currentDuration == null || currentDuration <= requireNotNull(initialDuration)) &&
                        SystemClock.elapsedRealtime() < progressionDeadline) {
                        SystemClock.sleep(100)
                        currentDuration = duration()
                    }
                    assertTrue("capture notification duration did not advance", currentDuration != null && currentDuration > requireNotNull(initialDuration))
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
            if (testMute) scenario.onActivity { activity ->
                val control = find(activity.window.decorView) { it is Switch && it.text.toString() == "Mute sending" } as Switch
                assertTrue("mute control remained active after stop", !control.isEnabled && !control.isChecked)
            }
            if (changeUsage) {
                var finished = false
                val finishDeadline = SystemClock.elapsedRealtime() + 10_000
                while (!finished && SystemClock.elapsedRealtime() < finishDeadline) {
                    scenario.onActivity { finished = LabStatus.captureAdmission.canBegin() }
                    if (!finished) SystemClock.sleep(100)
                }
                assertTrue("capture cleanup did not finish", finished)
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    find(root) { it is Button && it.text.toString() == "Stop test tone" }.performClick()
                    find(root) { it is Button && it.text.toString() == "Start test tone" }.performClick()
                    assertEquals(AudioAttributes.USAGE_GAME, activity.testToneUsage())
                    find(root) { it is Button && it.text.toString() == "Stop test tone" }.performClick()
                }
            }
            scenario.onActivity { activity ->
                val count = find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Sender queue drops: [0-9]+"))
                }
                assertTrue("final sender queue count was not retained", count is TextView)
                val sent = find(activity.window.decorView) {
                    it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                }
                assertTrue("final sent packet count was not retained", sent is TextView)
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

    private fun clickNodeById(automation: android.app.UiAutomation, id: String): Boolean {
        val deadline = SystemClock.elapsedRealtime() + 10_000
        while (SystemClock.elapsedRealtime() < deadline) {
            val node = automation.rootInActiveWindow?.findAccessibilityNodeInfosByViewId(id)?.firstOrNull()
            if (node != null && node.performAction(AccessibilityNodeInfo.ACTION_CLICK)) return true
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
