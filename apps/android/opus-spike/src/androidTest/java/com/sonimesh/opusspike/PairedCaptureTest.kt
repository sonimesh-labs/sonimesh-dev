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
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
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
    fun captures_tone_after_pairing_and_stops() = runCapture()

    @Test
    fun captures_after_refused_connection_is_retried() {
        assumeTrue("retry test requires a gated desktop listener", InstrumentationRegistry.getArguments().getString("retryGate") == "true")
        runCapture(expectConnectionRetry = true)
    }

    @Test
    fun captures_after_scanned_invitation() = runCapture(startFromScan = true)

    @Test
    fun captures_when_next_invitation_changes_during_consent() = runCapture(startFromScan = true, changeSetupDuringConsent = true)

    @Test
    fun captures_after_pasted_invitation() = runCapture(startFromPaste = true)

    @Test
    fun keeps_capturing_tone_after_activity_recreation() = runCapture(recreate = true)

    @Test
    fun tone_follows_active_capture_usage() = runCapture(changeUsage = true)

    @Test
    fun captures_again_after_stopping() {
        runCapture()
        runCapture()
    }

    @Test
    fun keeps_service_active_while_screen_is_off() = runCapture(screenOff = true)

    @Test
    fun reconnects_after_desktop_disconnect_without_restarting_capture() {
        assumeTrue("reconnect test requires --disconnect-after-ms with --keep-listening", InstrumentationRegistry.getArguments().getString("reconnectTest") == "true")
        runCapture(expectReconnect = true)
    }

    @Test
    fun stops_capture_during_reconnect_without_restarting_session() {
        assumeTrue("reconnect test requires --disconnect-after-ms with --keep-listening", InstrumentationRegistry.getArguments().getString("reconnectTest") == "true")
        runCapture(stopDuringReconnect = true)
    }

    @Test
    fun retries_capture_after_reconnect_attempts_are_exhausted() {
        assumeTrue("recovery test requires --disconnect-after-ms and --reject-starts with --keep-listening", InstrumentationRegistry.getArguments().getString("recoveryTest") == "true")
        runCapture(retryAfterReconnectExhaustion = true)
    }

    @Test
    fun returns_to_active_capture_from_notification() = runCapture(notificationReturn = true)

    @Test
    fun mutes_and_resumes_sending_during_capture() = runCapture(testMute = true)

    private fun runCapture(expectReconnect: Boolean = false, stopDuringReconnect: Boolean = false, retryAfterReconnectExhaustion: Boolean = false, screenOff: Boolean = false, notificationReturn: Boolean = false, startFromScan: Boolean = false, startFromPaste: Boolean = false, recreate: Boolean = false, changeUsage: Boolean = false, testMute: Boolean = false, expectConnectionRetry: Boolean = false, changeSetupDuringConsent: Boolean = false) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val args = InstrumentationRegistry.getArguments()
        var stopAcknowledgement = "unavailable"
        assumeTrue(
            "paired audio lab requires pairingInvitationHex or pairingSpkiHex",
            args.containsKey("pairingInvitationHex") || args.containsKey("pairingSpkiHex"),
        )
        val pin = args.getString("pairingSpkiHex")
        if (startFromScan) assumeTrue("scan test requires pairingInvitationHex", args.containsKey("pairingInvitationHex"))
        if (startFromPaste) assumeTrue("paste test requires pairingInvitationHex", args.containsKey("pairingInvitationHex"))
        ActivityScenario.launch<LabActivity>(Intent(instrumentation.targetContext, LabActivity::class.java)).use { scenario ->
            scenario.onActivity { activity ->
                (find(activity.window.decorView) {
                    it is Spinner && it.contentDescription == "Playback usage for capture"
                } as Spinner).setSelection(if (changeUsage) 1 else 0)
            }
            instrumentation.waitForIdleSync()
            (args.getString("bitrateKbps") ?: "96").let { requested ->
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
                (find(root) { it is EditText && it.contentDescription == "Desktop UDP port" } as EditText)
                    .setText(args.getString("mediaPort") ?: DEFAULT_LAB_MEDIA_PORT.toString())
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
            if (changeSetupDuringConsent) {
                scenario.onActivity { activity ->
                    activity.applyScannedInvitation("SONIMESH-LAB1:39095:${labHex(labInvitation("ab".repeat(32)))}")
                    val port = find(activity.window.decorView) { it is EditText && it.contentDescription == "Desktop UDP port" } as EditText
                    assertEquals("39095", port.text.toString())
                }
            }
            if (android.os.Build.VERSION.SDK_INT >= 33 &&
                instrumentation.targetContext.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                assertTrue("notification permission dialog missing", clickNodeById(automation, "com.android.permissioncontroller:id/permission_allow_button"))
            }
            assertTrue("screen-capture selector missing", clickNode(automation, "Share one app"))
            assertTrue("entire-screen option missing", clickNode(automation, "Share entire screen"))
            assertTrue("screen-capture consent missing", clickNode(automation, "Share screen"))
            if (expectConnectionRetry) {
                var retrying = false
                val retryDeadline = SystemClock.elapsedRealtime() + 5_000
                while (!retrying && SystemClock.elapsedRealtime() < retryDeadline) {
                    scenario.onActivity { activity ->
                        retrying = containsStatus(activity.window.decorView, "Status: Desktop listener is not ready. Retrying connection")
                    }
                    if (!retrying) SystemClock.sleep(50)
                }
                assertTrue("connection retry was not reported", retrying)
                val manager = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                assertEquals("Transport: paired UDP · Connecting to desktop",
                    manager.activeNotifications.firstOrNull { it.id == 1 }?.notification?.extras?.getString(Notification.EXTRA_TEXT))
                instrumentation.sendStatus(1, Bundle().apply { putString("pairedCapture", "retrying") })
                DatagramSocket().use { socket ->
                    val marker = byteArrayOf(1)
                    socket.send(DatagramPacket(marker, marker.size, InetAddress.getByName("10.0.2.2"), 47611))
                }
            }
            var capturing = false
            val deadline = SystemClock.elapsedRealtime() + 20_000
            while (!capturing && SystemClock.elapsedRealtime() < deadline) {
                scenario.onActivity { activity ->
                    capturing = containsStatus(activity.window.decorView, "Status: Capturing audio")
                }
                if (!capturing) SystemClock.sleep(100)
            }
            assertTrue("capture did not start", capturing)
            if (expectConnectionRetry) {
                val notification = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                    .activeNotifications.firstOrNull { it.id == 1 }?.notification
                assertEquals("Transport: paired UDP", notification?.extras?.getString(Notification.EXTRA_TEXT))
            }
            scenario.onActivity { activity ->
                val usage = if (changeUsage) "Media" else "All supported usages"
                val bitrate = args.getString("bitrateKbps") ?: "96"
                assertTrue(containsStatus(activity.window.decorView, "Session request: $usage, $bitrate kbps target"))
                val port = args.getString("expectedMediaPort") ?: args.getString("mediaPort") ?: DEFAULT_LAB_MEDIA_PORT.toString()
                find(activity.window.decorView) { it is TextView && it.text.toString().lineSequence().any { line -> line == "Requested UDP port: $port" } }
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
            if (expectReconnect || stopDuringReconnect || retryAfterReconnectExhaustion) {
                instrumentation.sendStatus(1, Bundle().apply { putString("pairedCapture", "ready") })
                if (expectReconnect) automation.executeShellCommand("input keyevent KEYCODE_HOME").close()
                var disconnected = false
                val disconnectDeadline = SystemClock.elapsedRealtime() + 20_000
                while (!disconnected && SystemClock.elapsedRealtime() < disconnectDeadline) {
                    if (expectReconnect) {
                        val notification = instrumentation.targetContext
                            .getSystemService(NotificationManager::class.java)
                            .activeNotifications.firstOrNull { it.id == 1 }?.notification
                        disconnected = notification?.extras?.getString(Notification.EXTRA_TEXT) ==
                            "Transport: paired UDP · Reconnecting to desktop"
                    } else {
                        scenario.onActivity { activity ->
                            disconnected = containsStatus(activity.window.decorView, "Status: Desktop connection lost. Reconnecting")
                        }
                    }
                    if (!disconnected) SystemClock.sleep(100)
                }
                assertTrue("desktop disconnect was not reported", disconnected)
                if (expectReconnect) {
                    val manager = instrumentation.targetContext.getSystemService(NotificationManager::class.java)
                    val notification = manager.activeNotifications.firstOrNull { it.id == 1 }?.notification
                    requireNotNull(notification?.contentIntent) { "reconnecting notification has no open action" }.send()
                    val returnDeadline = SystemClock.elapsedRealtime() + 10_000
                    var returned = false
                    while (!returned && SystemClock.elapsedRealtime() < returnDeadline) {
                        returned = automation.rootInActiveWindow?.packageName?.toString() ==
                            instrumentation.targetContext.packageName
                        if (!returned) SystemClock.sleep(100)
                    }
                    assertTrue("notification did not return to capture activity", returned)
                }
                if (retryAfterReconnectExhaustion) {
                    scenario.onActivity { activity ->
                        find(activity.window.decorView) { it is Button && it.text.toString() == "Stop test tone" }.performClick()
                    }
                    var silent = false
                    val silentDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (!silent && SystemClock.elapsedRealtime() < silentDeadline) {
                        scenario.onActivity { activity ->
                            val root = activity.window.decorView
                            silent = containsStatus(root, "Capture signal: no capturable audio")
                            assertTrue("signal change replaced reconnect status",
                                containsStatus(root, "Status: Desktop connection lost. Reconnecting"))
                        }
                        if (!silent) SystemClock.sleep(100)
                    }
                    assertTrue("capture signal did not report silence during reconnect", silent)
                    scenario.onActivity { activity ->
                        find(activity.window.decorView) { it is Button && it.text.toString() == "Start test tone" }.performClick()
                    }
                    var signal = false
                    val signalDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (!signal && SystemClock.elapsedRealtime() < signalDeadline) {
                        scenario.onActivity { activity ->
                            val root = activity.window.decorView
                            signal = containsStatus(root, "Capture signal: signal detected")
                            assertTrue("signal recovery replaced reconnect status",
                                containsStatus(root, "Status: Desktop connection lost. Reconnecting"))
                        }
                        if (!signal) SystemClock.sleep(100)
                    }
                    assertTrue("capture signal did not recover during reconnect", signal)
                    var retryAvailable = false
                    val retryDeadline = SystemClock.elapsedRealtime() + 60_000
                    while (!retryAvailable && SystemClock.elapsedRealtime() < retryDeadline) {
                        scenario.onActivity { activity ->
                            val root = activity.window.decorView
                            retryAvailable = LabStatus.captureAdmission.canBegin() &&
                                containsStatus(root, "Status: Desktop connection was not restored after 12 attempts") &&
                                runCatching {
                                    find(root) { it is Button && it.text.toString() == "Retry last capture" && it.visibility == View.VISIBLE }
                                }.isSuccess
                        }
                        if (!retryAvailable) SystemClock.sleep(100)
                    }
                    assertTrue("retry did not become available after reconnect attempts were exhausted", retryAvailable)
                    scenario.onActivity { activity ->
                        find(activity.window.decorView) {
                            it is Button && it.text.toString() == "Retry last capture"
                        }.performClick()
                    }
                    assertTrue("screen-capture selector missing after retry", clickNode(automation, "Share one app"))
                    assertTrue("entire-screen option missing after retry", clickNode(automation, "Share entire screen"))
                    assertTrue("screen-capture consent missing after retry", clickNode(automation, "Share screen"))
                    var capturingAgain = false
                    var retryStatus = "unavailable"
                    var retryAdmissionCanBegin = true
                    val captureDeadline = SystemClock.elapsedRealtime() + 20_000
                    while (!capturingAgain && SystemClock.elapsedRealtime() < captureDeadline) {
                        scenario.onActivity { activity ->
                            val root = activity.window.decorView
                            retryStatus = runCatching {
                                find(root) { it is TextView && it.text.toString().startsWith("Status:") }
                                    .let { (it as TextView).text.toString() }
                            }.getOrDefault("status unavailable")
                            retryAdmissionCanBegin = LabStatus.captureAdmission.canBegin()
                            capturingAgain = retryStatus.startsWith("Status: Capturing audio") && !retryAdmissionCanBegin
                        }
                        if (!capturingAgain) SystemClock.sleep(100)
                    }
                    assertTrue(
                        "manual retry did not start a new paired capture (status=$retryStatus, admissionCanBegin=$retryAdmissionCanBegin)",
                        capturingAgain,
                    )
                    var resumedMedia = false
                    val mediaDeadline = SystemClock.elapsedRealtime() + 10_000
                    while (!resumedMedia && SystemClock.elapsedRealtime() < mediaDeadline) {
                        scenario.onActivity { activity ->
                            val root = activity.window.decorView
                            val sent = find(root) {
                                it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                            } as TextView
                            resumedMedia = sent.text.toString().substringAfter(": ").toLong() >= 25 &&
                                containsStatus(root, "Capture signal: signal detected")
                        }
                        if (!resumedMedia) SystemClock.sleep(100)
                    }
                    assertTrue("manual retry did not resume sending captured signal", resumedMedia)
                    SystemClock.sleep(3_000)
                }
                if (stopDuringReconnect) {
                    scenario.onActivity { activity ->
                        val stop = find(activity.window.decorView) {
                            it is Button && it.text.toString() == "Stop capture"
                        } as Button
                        assertTrue("stop control is unavailable during reconnect", stop.isEnabled)
                        assertTrue("stop control rejected reconnect cancellation", stop.performClick())
                    }
                }
                if (!stopDuringReconnect && !retryAfterReconnectExhaustion) {
                    var observedDuration: Long? = null
                    val durationObserver: (Long?) -> Unit = { observedDuration = it }
                    scenario.onActivity { LabStatus.observeCaptureDuration(durationObserver) }
                    val durationBeforeReconnect = observedDuration ?: 0L
                    scenario.onActivity { activity ->
                        assertTrue("capture service stopped while reconnecting", !LabStatus.captureAdmission.canBegin())
                    }
                    var restored = false
                    val reconnectDeadline = SystemClock.elapsedRealtime() + 20_000
                    while (!restored && SystemClock.elapsedRealtime() < reconnectDeadline) {
                        scenario.onActivity { activity ->
                            restored = containsStatus(activity.window.decorView, "Status: Desktop connection restored.")
                        }
                        if (!restored) SystemClock.sleep(100)
                    }
                    assertTrue("desktop session did not recover", restored)
                    if (expectReconnect) {
                        val notification = instrumentation.targetContext
                            .getSystemService(NotificationManager::class.java)
                            .activeNotifications.firstOrNull { it.id == 1 }?.notification
                        assertEquals(
                            "Transport: paired UDP · Desktop reconnected",
                            notification?.extras?.getString(Notification.EXTRA_TEXT),
                        )
                    }
                    var durationAdvanced = false
                    val durationDeadline = SystemClock.elapsedRealtime() + 3_000
                    while (!durationAdvanced && SystemClock.elapsedRealtime() < durationDeadline) {
                        scenario.onActivity {
                            durationAdvanced = (observedDuration ?: 0L) > durationBeforeReconnect
                            assertTrue("capture service stopped after reconnect", !LabStatus.captureAdmission.canBegin())
                        }
                        if (!durationAdvanced) SystemClock.sleep(100)
                    }
                    scenario.onActivity { LabStatus.removeCaptureDuration(durationObserver) }
                    assertTrue("capture duration did not continue across reconnect", durationAdvanced)
                    var packetsBeforeReconnectDrain = 0L
                    scenario.onActivity { activity ->
                        val sent = find(activity.window.decorView) {
                            it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                        } as TextView
                        packetsBeforeReconnectDrain = sent.text.toString().substringAfter(": ").toLong()
                    }
                    SystemClock.sleep(3_000)
                    scenario.onActivity { activity ->
                        val sent = find(activity.window.decorView) {
                            it is TextView && it.text.toString().matches(Regex("Media packets sent: [0-9]+"))
                        } as TextView
                        val packetsAfterReconnectDrain = sent.text.toString().substringAfter(": ").toLong()
                        assertTrue(
                            "media packets did not continue after desktop reconnect",
                            packetsAfterReconnectDrain > packetsBeforeReconnectDrain,
                        )
                    }
                }
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
                    assertEquals(
                        "Transport: paired UDP · Sending muted",
                        manager.activeNotifications.firstOrNull { it.id == 1 }?.notification
                            ?.extras?.getString(Notification.EXTRA_TEXT),
                    )
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
                    assertEquals(
                        "Transport: paired UDP",
                        manager.activeNotifications.firstOrNull { it.id == 1 }?.notification
                            ?.extras?.getString(Notification.EXTRA_TEXT),
                    )
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
            }
            scenario.onActivity { activity ->
                val stopButton = find(activity.window.decorView) {
                    it is Button && it.text.toString() == "Stop capture"
                } as Button
                assertTrue("stop capture control is disabled", stopButton.isEnabled)
                assertTrue("stop capture control did not accept click", stopButton.performClick())
            }
            var stopAccepted = false
            var stopAlreadyCompleted = false
            val stopAcceptedDeadline = SystemClock.elapsedRealtime() + 3_000
            while (!stopAccepted && SystemClock.elapsedRealtime() < stopAcceptedDeadline) {
                scenario.onActivity { activity ->
                    val status = find(activity.window.decorView) {
                        it is TextView && it.text.toString().startsWith("Status:")
                    } as TextView
                    stopAcknowledgement = status.text.toString()
                    stopAlreadyCompleted = stopAcknowledgement.contains("Capture stopped") &&
                        LabStatus.captureAdmission.canBegin()
                    stopAccepted = stopAcknowledgement.contains("Stopping capture") || stopAlreadyCompleted
                }
                if (!stopAccepted) SystemClock.sleep(50)
            }
            assertTrue("service did not acknowledge stop", stopAccepted)
            if (!stopAlreadyCompleted) {
                scenario.onActivity { activity ->
                    val stopButton = find(activity.window.decorView) {
                        it is Button && it.text.toString() == "Stop capture"
                    } as Button
                    assertTrue("repeat stop capture control did not accept click", stopButton.performClick())
                }
            }
            var stopped = false
            var statusAtStopDeadline = "unavailable"
            var publishedStatusAtStopDeadline = "unavailable"
            val stopStatusObserver: (String) -> Unit = { publishedStatusAtStopDeadline = it }
            scenario.onActivity { LabStatus.observe(stopStatusObserver) }
            var serviceAtStopDeadline = true
            var cleanupReadyAtStopDeadline = false
            var workerStacksAtStopDeadline = "unavailable"
            val stopDeadline = SystemClock.elapsedRealtime() + 10_000
            while (!stopped && SystemClock.elapsedRealtime() < stopDeadline) {
                scenario.onActivity { activity ->
                    @Suppress("DEPRECATION")
                    serviceAtStopDeadline = activity.getSystemService(ActivityManager::class.java)
                        .getRunningServices(Int.MAX_VALUE).any { it.service.className == LabCaptureService::class.java.name }
                    cleanupReadyAtStopDeadline = LabStatus.captureAdmission.canBegin()
                    stopped = !serviceAtStopDeadline && cleanupReadyAtStopDeadline
                    if (!stopped) {
                        statusAtStopDeadline = runCatching {
                            find(activity.window.decorView) {
                                it is TextView && it.text.toString().startsWith("Status:")
                            }.let { (it as TextView).text.toString() }
                        }.getOrDefault("status unavailable")
                        if (stopDeadline - SystemClock.elapsedRealtime() <= 250) {
                            workerStacksAtStopDeadline = Thread.getAllStackTraces().entries
                                .filter { it.key.name.startsWith("audio-lab-") }
                                .joinToString("\n") { (thread, stack) ->
                                    "${thread.name} (${thread.state}, interrupted=${thread.isInterrupted}): ${stack.take(8).joinToString(" <- ")}"
                                }
                        }
                    }
                }
                if (!stopped) SystemClock.sleep(100)
            }
            scenario.onActivity { LabStatus.remove(stopStatusObserver) }
            assertTrue(
                "capture service did not stop (servicePresent=$serviceAtStopDeadline, cleanupReady=$cleanupReadyAtStopDeadline, publishedStatus=$publishedStatusAtStopDeadline, stopAcknowledgement=$stopAcknowledgement); $statusAtStopDeadline\n$workerStacksAtStopDeadline",
                stopped,
            )
            val pairedWorkersDeadline = SystemClock.elapsedRealtime() + 2_000
            var pairedWorkersStopped = false
            while (!pairedWorkersStopped && SystemClock.elapsedRealtime() < pairedWorkersDeadline) {
                pairedWorkersStopped = Thread.getAllStackTraces().keys.none {
                    it.name == "audio-lab-paired-session" || it.name == "audio-lab-paired-sender"
                }
                if (!pairedWorkersStopped) SystemClock.sleep(50)
            }
            assertTrue("paired control or media worker remained alive after service cleanup", pairedWorkersStopped)
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
            scenario.onActivity { activity ->
                assertTrue("capture did not publish completion", containsStatus(activity.window.decorView, "Status: Capture stopped."))
                val rate = find(activity.window.decorView) {
                    it is TextView && if (retryAfterReconnectExhaustion) {
                        it.text.toString().startsWith("Encoded payload average: ")
                    } else {
                        it.text.toString().matches(Regex("Encoded payload average: [0-9]+ kbps"))
                    }
                } as TextView
                instrumentation.sendStatus(1, Bundle().apply { putString("encodedPayload", rate.text.toString()) })
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
