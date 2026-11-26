package com.sonimesh.opusspike

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.graphics.Rect
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.Spinner
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.sonimesh.sender.encoder.OpusBitrate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LabStatusTest {
    @Test
    fun changingPinLetterCaseRetainsImportedInvitation() {
        val invitation = labHex(labInvitation("ab".repeat(32), "127.0.0.1", 47612))
        val pendingField = LabActivity::class.java.getDeclaredField("pendingInvitation").apply { isAccessible = true }
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    requireNotNull(findTextField(root, "Lab invitation payload")).text = invitation
                    requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                    requireNotNull(findTextField(root, "Desktop IPv4 address for UDP")).text = "10.0.2.2"
                    requireNotNull(findTextField(root, "Paired lab desktop SPKI")).text = "AB".repeat(32)
                    requireNotNull(findButton(root, "Start paired UDP capture")).performClick()
                    assertEquals(invitation, activity.getPreferences(Activity.MODE_PRIVATE).getString("labInvitation", null))
                    assertEquals(invitation, labHex(pendingField.get(activity) as ByteArray))
                    assertTrue(!hasStatus(root, "Connection settings changed; using the manual lab setup"))
                    LabStatus.captureAdmission.launchFailed()
                    requireNotNull(findButton(root, "Clear saved lab setup")).performClick()
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.captureAdmission.launchFailed()
            }
        }
    }

    @Test
    fun changedControlPortReplacesImportedInvitationAndIsSaved() {
        val invitation = labHex(labInvitation("ab".repeat(32), "127.0.0.1", 47612))
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    requireNotNull(findTextField(root, "Lab invitation payload")).text = invitation
                    requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                    requireNotNull(findTextField(root, "Desktop IPv4 address for UDP")).text = "10.0.2.2"
                    val controlPort = requireNotNull(findTextField(root, "Paired control TCP port"))
                    controlPort.text = "0"
                    requireNotNull(findButton(root, "Start paired UDP capture")).performClick()
                    assertTrue(hasStatus(root, "Status: Enter a paired control TCP port from 1 to 65535."))
                    assertTrue(activity.getPreferences(Activity.MODE_PRIVATE).contains("labInvitation"))
                    controlPort.text = "47613"
                    requireNotNull(findButton(root, "Start paired UDP capture")).performClick()
                    assertTrue(!activity.getPreferences(Activity.MODE_PRIVATE).contains("labInvitation"))
                    assertEquals(47613, activity.getPreferences(Activity.MODE_PRIVATE).getInt("labControlPort", 0))
                    assertTrue(hasStatus(root, "Connection settings changed; using the manual lab setup instead of the imported invitation."))
                    LabStatus.captureAdmission.launchFailed()
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    assertEquals("47613", requireNotNull(findTextField(root, "Paired control TCP port")).text.toString())
                    requireNotNull(findButton(root, "Clear saved lab setup")).performClick()
                    assertEquals("47610", requireNotNull(findTextField(root, "Paired control TCP port")).text.toString())
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.captureAdmission.launchFailed()
            }
        }
    }

    @Test
    fun customControlPortInvitationSurvivesActivityRecreation() {
        val invitation = labHex(labInvitation("ab".repeat(32), "127.0.0.1", 47612))
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                requireNotNull(findTextField(root, "Lab invitation payload")).text = invitation
                requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                assertTrue(hasStatus(root, "Status: Invitation applied. Check the desktop IPv4 for UDP separately. Control TCP port 47612."))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                val field = LabActivity::class.java.getDeclaredField("importedInvitation").apply { isAccessible = true }
                assertEquals(invitation, labHex(field.get(activity) as ByteArray))
                requireNotNull(findButton(activity.window.decorView, "Clear saved lab setup")).performClick()
            }
        }
    }

    @Test
    fun pairedCaptureStartIsVisibleAndValidatesTheCurrentSetup() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().waitForIdleSync()
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val start = requireNotNull(findButton(root, "Start paired UDP capture"))
                val visible = Rect()
                assertTrue("paired start should be visible before scrolling", start.getGlobalVisibleRect(visible))
                assertEquals(start.height, visible.height())
                requireNotNull(findTextField(root, "Desktop IPv4 address for UDP")).text = ""
                start.performClick()
                assertTrue(hasStatus(root, "Status: Enter a valid desktop IPv4 address for UDP."))
                assertTrue(LabStatus.captureAdmission.canBegin())
            }
        }
    }

    @Test
    fun stopCancelsPendingProjectionAndIgnoresItsResult() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        val pendingField = LabActivity::class.java.getDeclaredField("pendingInvitation").apply { isAccessible = true }
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    assertTrue(LabStatus.captureAdmission.begin(false))
                    pendingField.set(activity, byteArrayOf(1, 2))
                    requireNotNull(findButton(activity.window.decorView, "Stop capture")).performClick()
                    assertEquals(null, pendingField.get(activity))
                    assertTrue(LabStatus.captureAdmission.cancellationPending())
                    assertTrue(!LabStatus.captureAdmission.canBegin())
                    deliverScannerResult(activity, 2, Activity.RESULT_OK, null)
                    assertTrue(LabStatus.captureAdmission.canBegin())
                    assertTrue(hasStatus(activity.window.decorView, "Status: Capture start cancelled."))
                    assertTrue(LabStatus.captureAdmission.begin(false))
                    LabStatus.captureAdmission.launchFailed()
                }
            }
        } finally {
            instrumentation.runOnMainSync { LabStatus.captureAdmission.launchFailed() }
        }
    }

    @Test
    fun pending_invitation_survives_recreation_and_is_cleared_on_denial() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        val original = labHex(labInvitation("ab".repeat(32)))
        val replacement = labHex(labInvitation("cd".repeat(32)))
        val pendingField = LabActivity::class.java.getDeclaredField("pendingInvitation").apply { isAccessible = true }
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    activity.applyScannedInvitation(original)
                    assertTrue(LabStatus.captureAdmission.begin(false))
                    pendingField.set(activity, labHexToBytes(original))
                    assertEquals(original, labHex(pendingField.get(activity) as ByteArray))
                    activity.applyScannedInvitation(replacement)
                    assertEquals(original, labHex(pendingField.get(activity) as ByteArray))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertEquals(original, labHex(pendingField.get(activity) as ByteArray))
                    deliverScannerResult(activity, 2, Activity.RESULT_CANCELED, null)
                    assertEquals(null, pendingField.get(activity))
                    assertTrue(LabStatus.captureAdmission.canBegin())
                }
            }
        } finally {
            instrumentation.runOnMainSync { LabStatus.captureAdmission.launchFailed() }
        }
    }

    @Test
    fun setup_feedback_does_not_replace_active_capture_status() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD, LabTransport.PairedUdp, 39093)
                    LabStatus.publish("Capturing audio and sending to desktop.")
                    val invitation = "SONIMESH-LAB1:39094:${labHex(labInvitation("ab".repeat(32)))}"
                    requireNotNull(findTextField(root, "Lab invitation payload")).text = invitation
                    requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                    assertTrue(hasStatus(root, "Status: Capturing audio and sending to desktop."))
                    assertTrue(hasStatus(root, "Next setup: Invitation applied. UDP port set to 39094."))
                    assertTrue(hasStatus(root, "Session request: All supported usages, 96 kbps target\nTransport: paired UDP\nRequested UDP port: 39093"))
                    activity.applyScannedInvitation("invalid")
                    assertTrue(hasStatus(root, "Status: Capturing audio and sending to desktop."))
                    assertTrue(hasStatus(root, "Next setup: Paste a valid lab invitation payload."))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    assertTrue(hasStatus(root, "Status: Capturing audio and sending to desktop."))
                    assertTrue(hasStatus(root, "Next setup: Paste a valid lab invitation payload."))
                    LabStatus.publish("Desktop connection ended.")
                    assertTrue(hasStatus(root, "Status: Desktop connection ended."))
                }
            }
        } finally {
            instrumentation.runOnMainSync { LabStatus.serviceFinished() }
        }
    }

    @Test
    fun sessionRequestSurvivesRecreationAndStop() {
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Game, OpusBitrate.fromBitsPerSecond(128_000))
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Game, 128 kbps target"))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Game, 128 kbps target"))
                    LabStatus.serviceFinished()
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Game, 128 kbps target"))
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.serviceFinished()
            }
        }
    }

    @Test
    fun testToneButtonReflectsPlaybackAndActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val button = requireNotNull(findButton(activity.window.decorView, "Start test tone"))
                button.performClick()
                assertTrue(button.text.toString() == "Stop test tone")
                button.performClick()
                assertTrue(button.text.toString() == "Start test tone")
                button.performClick()
                assertTrue(button.text.toString() == "Stop test tone")
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(findButton(activity.window.decorView, "Stop test tone") != null)
                assertTrue(activity.isTestTonePlaying())
                findButton(activity.window.decorView, "Stop test tone")?.performClick()
            }
        }
    }

    @Test
    fun invalid_scan_does_not_start_capture() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                deliverScannerResult(activity, 4, Activity.RESULT_CANCELED, "invalid")
                assertTrue(LabStatus.captureAdmission.canBegin())
                deliverScannerResult(activity, 4, Activity.RESULT_OK, "invalid")
                assertTrue(hasStatus(activity.window.decorView, "Status: Paste a valid lab invitation payload."))
                assertTrue(LabStatus.captureAdmission.canBegin())
                deliverScannerResult(activity, 4, Activity.RESULT_OK, null)
                assertTrue(hasStatus(activity.window.decorView, "Status: Scan a valid lab invitation"))
                assertTrue(LabStatus.captureAdmission.canBegin())
            }
        }
    }
    @Test
    fun senderQueueDropsSurviveActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publishSenderQueueDrops(7)
                LabStatus.publishSentPackets(12)
                LabStatus.publishEncoderRate(73)
                assertTrue(hasStatus(activity.window.decorView, "Sender queue drops: 7"))
                assertTrue(hasStatus(activity.window.decorView, "Media packets sent: 12"))
                assertTrue(hasStatus(activity.window.decorView, "Encoded payload average: 73 kbps"))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Sender queue drops: 7"))
                assertTrue(hasStatus(activity.window.decorView, "Media packets sent: 12"))
                assertTrue(hasStatus(activity.window.decorView, "Encoded payload average: 73 kbps"))
                LabStatus.publishSenderQueueDrops(null)
                LabStatus.publishSentPackets(null)
                LabStatus.publishEncoderRate(null)
            }
        }
    }

    @Test
    fun lastSendIntervalSurvivesRecreationAndStopButClearsForNewCapture() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            try {
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    LabStatus.publishSenderInterval(SenderInterval(12, 1050))
                    assertTrue(hasStatus(activity.window.decorView, "Last local send interval: 12 packets in 1050 ms"))
                    assertTrue(LabStatus.copyableEvidence().contains("Last local send interval: 12 packets in 1050 ms"))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Last local send interval: 12 packets in 1050 ms"))
                    LabStatus.serviceFinished()
                    assertTrue(hasStatus(activity.window.decorView, "Last local send interval: 12 packets in 1050 ms"))
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    assertTrue(hasStatus(activity.window.decorView, "Last local send interval: —"))
                }
            } finally {
                scenario.onActivity { LabStatus.serviceFinished() }
            }
        }
    }

    @Test
    fun senderGainIsRestoredAndResetsForNewCapture() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            try {
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    LabStatus.publishSenderGain(150)
                    assertTrue(hasStatus(activity.window.decorView, "Sender gain: 150%"))
                    assertTrue(LabStatus.copyableEvidence().contains("Sender gain (last known): 150%"))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Sender gain: 150%"))
                    LabStatus.serviceFinished()
                    assertTrue(hasStatus(activity.window.decorView, "Sender gain: —"))
                    assertTrue(LabStatus.copyableEvidence().contains("Sender gain (last known): 150%"))
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    assertTrue(hasStatus(activity.window.decorView, "Sender gain: 100%"))
                }
            } finally {
                scenario.onActivity { LabStatus.serviceFinished() }
            }
        }
    }

    @Test
    fun clippingCountSurvivesRecreationAndStopButResetsForNewCapture() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            try {
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    LabStatus.publishClippedFrames(3)
                    assertTrue(hasStatus(activity.window.decorView, "Frames clipped by sender gain: 3. Lower sender gain if distortion is audible."))
                    assertTrue(LabStatus.copyableEvidence().contains("Frames clipped by sender gain: 3"))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Frames clipped by sender gain: 3. Lower sender gain if distortion is audible."))
                    LabStatus.serviceFinished()
                    assertTrue(LabStatus.copyableEvidence().contains("Frames clipped by sender gain: 3"))
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    assertTrue(hasStatus(activity.window.decorView, "Frames clipped by sender gain: —"))
                }
            } finally {
                scenario.onActivity { LabStatus.serviceFinished() }
            }
        }
    }

    @Test
    fun sentPacketStatusAllowsSaturatedLongValue() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publishSentPackets(Long.MAX_VALUE)
                assertTrue(hasStatus(activity.window.decorView, "Media packets sent: ${Long.MAX_VALUE}"))
                LabStatus.publishSentPackets(null)
            }
        }
    }

    @Test
    fun longStatusMessagesAreBoundedForTheScreenAndEvidence() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publish("${"x".repeat(400)}")
                val status = requireNotNull(findStatusText(activity.window.decorView))
                assertTrue(status.text.length <= 264)
                assertTrue(status.text.endsWith("..."))
                val evidence = LabStatus.copyableEvidence()
                val evidenceStatus = requireNotNull(evidence.lineSequence().firstOrNull { it.startsWith("Status: ") })
                assertTrue(evidenceStatus.length <= 264)
                assertTrue(evidenceStatus.endsWith("..."))
                LabStatus.publish("Idle")
            }
        }
    }

    @Test
    fun copyLabEvidenceIncludesBoundedStatusCounters() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                try {
                    LabStatus.serviceStarted(LabCaptureUsage.Game, OpusBitrate.fromBitsPerSecond(128_000))
                    LabStatus.publishSignal(LabSignalState.Silent)
                    LabStatus.publishCaptureDuration(42)
                    LabStatus.publishSenderQueueDrops(7)
                    LabStatus.publishSentPackets(12)
                    LabStatus.publishSenderTiming(SenderTimingSnapshot(11, 3))
                    LabStatus.publishEncoderRate(73)
                    LabStatus.publishSenderMuted(true)
                    requireNotNull(findButton(activity.window.decorView, "Copy lab evidence")).performClick()

                    val clip = requireNotNull(
                        activity.getSystemService(ClipboardManager::class.java).primaryClip,
                    )
                    val text = requireNotNull(clip.getItemAt(0).coerceToText(activity)).toString()
                    assertTrue(text.contains("Session request: Game, 128 kbps target"))
                    assertTrue(text.contains("Transport: ADB TCP"))
                    assertTrue(text.contains("Capture signal: no capturable audio"))
                    assertTrue(text.contains("Capture duration: 42s"))
                    assertTrue(text.contains("Sending mute (last known): on"))
                    assertTrue(text.contains("Sender queue drops: 7"))
                    assertTrue(text.contains("Media packets sent: 12"))
                    assertTrue(text.contains("Send intervals under 10 ms: 3 of 11"))
                    assertTrue(text.contains("Encoded payload average: 73 kbps"))
                    LabStatus.serviceFinished()
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    assertTrue(LabStatus.copyableEvidence().contains("Send intervals under 10 ms: —"))
                } finally {
                    LabStatus.serviceFinished()
                    LabStatus.publishSenderQueueDrops(null)
                    LabStatus.publishSentPackets(null)
                    LabStatus.publishSenderTiming(null)
                    LabStatus.publishEncoderRate(null)
                    LabStatus.publishSignal(null)
                    LabStatus.publishCaptureDuration(null)
                }
            }
        }
    }

    @Test
    fun evidenceRetainsLastMuteAfterStopAndResetsOnNextSession() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        instrumentation.runOnMainSync {
            try {
                LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD)
                assertTrue(LabStatus.copyableEvidence().contains("Sending mute (last known): off"))
                LabStatus.publishSenderMuted(true)
                LabStatus.serviceFinished()
                assertTrue(LabStatus.copyableEvidence().contains("Sending mute (last known): on"))
                LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD)
                assertTrue(LabStatus.copyableEvidence().contains("Sending mute (last known): off"))
            } finally {
                LabStatus.serviceFinished()
            }
        }
    }

    @Test
    fun captureSignalSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publishSignal(LabSignalState.Detected)
                assertTrue(hasStatus(activity.window.decorView, "Capture signal: signal detected"))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Capture signal: signal detected"))
                LabStatus.publishSignal(null)
            }
        }
    }

    @Test
    fun captureDurationSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publishCaptureDuration(7)
                assertTrue(hasStatus(activity.window.decorView, "Capture duration: 7s"))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Capture duration: 7s"))
                LabStatus.publishCaptureDuration(null)
            }
        }
    }

    @Test
    fun sessionTransportAppearsInTheScreenAndEvidence() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                try {
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD, LabTransport.PairedUdp)
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Media, 96 kbps target\nTransport: paired UDP"))
                    val evidence = LabStatus.copyableEvidence()
                    assertTrue(evidence.contains("Transport: paired UDP"))
                } finally {
                    LabStatus.serviceFinished()
                }
            }
        }
    }

    @Test
    fun udpSessionPortRemainsVisibleWhenNextSettingChangesAndAfterRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            try {
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD, LabTransport.PairedUdp, 39094)
                    requireNotNull(findTextField(activity.window.decorView, "Desktop UDP port")).setText("39095")
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Media, 96 kbps target\nTransport: paired UDP\nRequested UDP port: 39094"))
                    assertTrue(LabStatus.copyableEvidence().contains("Requested UDP port: 39094"))
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Media, 96 kbps target\nTransport: paired UDP\nRequested UDP port: 39094"))
                    LabStatus.serviceFinished()
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    assertTrue(hasStatus(activity.window.decorView, "Session request: Media, 96 kbps target\nTransport: ADB TCP"))
                    assertTrue(!LabStatus.copyableEvidence().contains("Requested UDP port:"))
                }
            } finally {
                scenario.onActivity { LabStatus.serviceFinished() }
            }
        }
    }

    @Test
    fun transportFlagsPreferPairedUdpAndDistinguishTheThreeCapturePaths() {
        assertEquals(LabTransport.Adb, LabTransport.fromFlags(udpMode = false, pairedMode = false))
        assertEquals(LabTransport.Udp, LabTransport.fromFlags(udpMode = true, pairedMode = false))
        assertEquals(LabTransport.PairedUdp, LabTransport.fromFlags(udpMode = true, pairedMode = true))
        assertEquals(LabTransport.PairedUdp, LabTransport.fromFlags(udpMode = false, pairedMode = true))
    }

    @Test
    fun retryUsesLastTransportAfterClosingAndReopeningActivity() {
        for (transport in listOf(LabTransport.Udp, LabTransport.PairedUdp)) {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity {
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD, transport)
                    LabStatus.serviceFinished()
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    requireNotNull(findTextField(root, "Desktop IPv4 address for UDP"))
                        .text = if (transport == LabTransport.Udp) "" else "10.0.2.2"
                    requireNotNull(findTextField(root, "Paired control IPv4 address")).text = ""
                    val retry = requireNotNull(findButton(root, "Retry capture with current setup"))
                    assertEquals(View.VISIBLE, retry.visibility)
                    retry.performClick()
                    val expected = if (transport == LabTransport.Udp) "Status: Enter a valid desktop IPv4 address for UDP."
                        else "Status: Enter a valid paired control IPv4 address."
                    assertTrue("retry must validate the last transport", hasStatus(root, expected))
                    assertTrue("failed validation must allow another attempt", LabStatus.captureAdmission.canBegin())
                }
            }
        }
    }

    @Test
    fun captureSelectorsPersistForTheNextLabLaunch() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                findSpinner(activity.window.decorView, "Playback usage for capture").setSelection(2)
                findSpinner(activity.window.decorView, "Opus target bitrate for capture").setSelection(2)
            }
        }
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                assertEquals("Game", findSpinner(activity.window.decorView, "Playback usage for capture").selectedItem)
                assertEquals("128 kbps", findSpinner(activity.window.decorView, "Opus target bitrate for capture").selectedItem)
                findSpinner(activity.window.decorView, "Playback usage for capture").setSelection(0)
                findSpinner(activity.window.decorView, "Opus target bitrate for capture").setSelection(1)
            }
        }
    }

    @Test
    fun labEvidenceShareIntentUsesPlainText() {
        val evidence = "SoniMesh paired audio lab\nStatus: Ready"
        val intent = buildLabEvidenceShareIntent(evidence)

        assertEquals(Intent.ACTION_SEND, intent.action)
        assertEquals("text/plain", intent.type)
        assertEquals(evidence, intent.getStringExtra(Intent.EXTRA_TEXT))
    }

    @Test
    fun pasteLabInvitationFillsTheInvitationField() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                activity.getSystemService(ClipboardManager::class.java).setPrimaryClip(
                    ClipData.newPlainText("invitation", "01ab"),
                )
                requireNotNull(findButton(activity.window.decorView, "Paste lab invitation")).performClick()
                assertTrue(findTextField(activity.window.decorView, "Lab invitation payload")?.text?.toString() == "01ab")
                assertTrue(hasStatus(activity.window.decorView, "Status: Invitation pasted. Apply it to continue."))
            }
        }
    }

    @Test
    fun pasteDesktopSpkiFillsThePairingField() {
        val spki = "ab".repeat(32)
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                activity.getSystemService(ClipboardManager::class.java).setPrimaryClip(
                    ClipData.newPlainText("spki", spki),
                )
                requireNotNull(findButton(activity.window.decorView, "Paste desktop SPKI")).performClick()
                assertTrue(findTextField(activity.window.decorView, "Paired lab desktop SPKI")?.text?.toString() == spki)
                assertTrue(hasStatus(activity.window.decorView, "Status: Desktop SPKI pasted."))
            }
        }
    }

    @Test
    fun sharedInvitationFillsTheFieldWithoutStartingCapture() {
        val intent = Intent(ApplicationProvider.getApplicationContext(), LabActivity::class.java).apply {
            action = Intent.ACTION_SEND
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, "01ab")
        }
        ActivityScenario.launch<LabActivity>(intent).use { scenario ->
            scenario.onActivity { activity ->
                assertTrue(findTextField(activity.window.decorView, "Lab invitation payload")?.text?.toString() == "01ab")
                assertTrue(hasStatus(activity.window.decorView, "Status: Invitation received from share."))
                assertTrue(LabStatus.captureAdmission.canBegin())
            }
        }
    }

    @Test
    fun sharedNonHexTextIsRejectedBeforeFilling() {
        val intent = Intent(ApplicationProvider.getApplicationContext(), LabActivity::class.java).apply {
            action = Intent.ACTION_SEND
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, "not-an-invitation")
        }
        ActivityScenario.launch<LabActivity>(intent).use { scenario ->
            scenario.onActivity { activity ->
                assertTrue(findTextField(activity.window.decorView, "Lab invitation payload")?.text?.isEmpty() == true)
                assertTrue(hasStatus(activity.window.decorView, "Status: Paste a valid lab invitation payload."))
            }
        }
    }

    @Test
    fun appliedInvitationRestoresAcrossActivityRecreationAndCanBeCleared() {
        val pin = "ab".repeat(32)
        val invitation = labHex(labInvitation(pin, "127.0.0.1"))
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                requireNotNull(findTextField(activity.window.decorView, "Lab invitation payload")).text = invitation
                requireNotNull(findButton(activity.window.decorView, "Apply invitation payload")).performClick()
                assertTrue(findTextField(activity.window.decorView, "Paired lab desktop SPKI")?.text?.toString() == pin)
                assertTrue(hasStatus(activity.window.decorView, "Status: Invitation applied."))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(findTextField(activity.window.decorView, "Paired lab desktop SPKI")?.text?.toString() == pin)
                assertTrue(findTextField(activity.window.decorView, "Paired control IPv4 address")?.text?.toString() == "127.0.0.1")
                requireNotNull(findButton(activity.window.decorView, "Clear saved lab setup")).performClick()
                assertTrue(findTextField(activity.window.decorView, "Paired lab desktop SPKI")?.text?.isEmpty() == true)
                assertTrue(hasStatus(activity.window.decorView, "Status: Saved lab setup cleared."))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(findTextField(activity.window.decorView, "Paired lab desktop SPKI")?.text?.isEmpty() == true)
            }
        }
    }

    @Test
    fun restoredSetupDoesNotReplaceCaptureOrFailureStatus() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val invitation = labHex(labInvitation("ab".repeat(32), "127.0.0.1"))
                    requireNotNull(findTextField(activity.window.decorView, "Lab invitation payload")).text = invitation
                    requireNotNull(findButton(activity.window.decorView, "Apply invitation payload")).performClick()
                    LabStatus.publish("Idle")
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Status: Restored the last lab setup."))
                    LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD, LabTransport.PairedUdp)
                    LabStatus.publish("Capturing audio and sending to desktop.")
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Status: Capturing audio and sending to desktop."))
                    LabStatus.publish("Desktop connection ended. Check the listener, then start capture again.")
                    LabStatus.serviceFinished()
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    assertTrue(hasStatus(activity.window.decorView, "Status: Desktop connection ended."))
                    assertEquals(View.VISIBLE, requireNotNull(findButton(activity.window.decorView, "Retry capture with current setup")).visibility)
                    requireNotNull(findButton(activity.window.decorView, "Clear saved lab setup")).performClick()
                }
            }
        } finally {
            instrumentation.runOnMainSync { LabStatus.serviceFinished() }
        }
    }

    @Test
    fun pairedRetryRetainsInvitationUntilManualSetupReplacesIt() {
        val instrumentation = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation()
        val original = labHex(labInvitation("ab".repeat(32), "127.0.0.1"))
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    requireNotNull(findTextField(root, "Lab invitation payload")).text = original
                    requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                    LabActivity::class.java.getDeclaredField("pairedMode").apply { isAccessible = true }.setBoolean(activity, true)
                    LabActivity::class.java.getDeclaredField("udpMode").apply { isAccessible = true }.setBoolean(activity, true)
                    assertTrue(LabStatus.captureAdmission.begin(false))
                    deliverScannerResult(activity, 2, Activity.RESULT_OK, null)
                    val field = LabActivity::class.java.getDeclaredField("importedInvitation").apply { isAccessible = true }
                    assertEquals(original, labHex(field.get(activity) as ByteArray))
                }
                val deadline = SystemClock.elapsedRealtime() + 10_000
                var ready = false
                while (!ready && SystemClock.elapsedRealtime() < deadline) {
                    scenario.onActivity { ready = LabStatus.captureAdmission.canBegin() }
                    if (!ready) SystemClock.sleep(50)
                }
                assertTrue("failed service did not finish", ready)
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    requireNotNull(findTextField(root, "Desktop IPv4 address for UDP")).text = "10.0.2.2"
                    requireNotNull(findTextField(root, "Paired lab desktop SPKI")).text = "cd".repeat(32)
                    requireNotNull(findButton(root, "Retry capture with current setup")).performClick()
                    assertTrue(!activity.getPreferences(Activity.MODE_PRIVATE).contains("labInvitation"))
                    LabStatus.captureAdmission.launchFailed()
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    val field = LabActivity::class.java.getDeclaredField("importedInvitation").apply { isAccessible = true }
                    assertEquals(null, field.get(activity))
                    requireNotNull(findButton(activity.window.decorView, "Clear saved lab setup")).performClick()
                }
            }
        } finally {
            instrumentation.runOnMainSync { LabStatus.captureAdmission.launchFailed() }
        }
    }

    @Test
    fun restoredInvitationUsesItsOwnPinWhenSavedManualSetupDiffers() {
        val invitation = labHex(labInvitation("ab".repeat(32), "127.0.0.1"))
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    activity.getPreferences(Activity.MODE_PRIVATE).edit()
                        .putString("labInvitation", invitation)
                        .putString("labPin", "cd".repeat(32))
                        .putString("labHost", "127.0.0.1")
                        .putString("desktopAddress", "10.0.2.2")
                        .commit()
                }
            }
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    requireNotNull(findButton(activity.window.decorView, "Start paired UDP capture")).performClick()
                    assertTrue(!activity.getPreferences(Activity.MODE_PRIVATE).contains("labInvitation"))
                    LabStatus.captureAdmission.launchFailed()
                    requireNotNull(findButton(activity.window.decorView, "Clear saved lab setup")).performClick()
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.captureAdmission.launchFailed()
            }
        }
    }

    @Test
    fun failedServiceStartSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                activity.startForegroundService(Intent(activity, LabCaptureService::class.java))
            }
            val deadline = SystemClock.elapsedRealtime() + 5_000
            var found = false
            while (!found && SystemClock.elapsedRealtime() < deadline) {
                scenario.onActivity { activity ->
                    found = hasStatus(activity.window.decorView, "Status: Unable to start capture.")
                }
                if (!found) SystemClock.sleep(50)
            }
            assertTrue("service failure should be visible", found)
            var retryVisible = false
            while (!retryVisible && SystemClock.elapsedRealtime() < deadline) {
                scenario.onActivity { activity ->
                    retryVisible = findButton(activity.window.decorView, "Retry capture with current setup")?.visibility == View.VISIBLE
                }
                if (!retryVisible) SystemClock.sleep(50)
            }
            assertTrue("retry should appear after failed service cleanup", retryVisible)
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Status: Unable to start capture."))
                assertEquals(View.VISIBLE, findButton(activity.window.decorView, "Retry capture with current setup")?.visibility)
            }
        }
    }

    private fun findButton(view: View, label: String): Button? {
        if (view is Button && view.text.toString() == label) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findButton(view.getChildAt(index), label)?.let { return it }
            }
        }
        return null
    }

    private fun findTextField(view: View, description: String): TextView? {
        if (view is TextView && view.contentDescription == description) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findTextField(view.getChildAt(index), description)?.let { return it }
            }
        }
        return null
    }

    private fun findSpinner(view: View, description: String): Spinner {
        if (view is Spinner && view.contentDescription == description) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                runCatching { findSpinner(view.getChildAt(index), description) }.getOrNull()?.let { return it }
            }
        }
        error("spinner not found: $description")
    }

    private fun hasStatus(view: View, prefix: String): Boolean {
        if (view is TextView && view.text.toString().startsWith(prefix)) return true
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                if (hasStatus(view.getChildAt(index), prefix)) return true
            }
        }
        return false
    }

    private fun findStatusText(view: View): TextView? {
        if (view is TextView && view.text.toString().startsWith("Status: ")) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                findStatusText(view.getChildAt(index))?.let { return it }
            }
        }
        return null
    }
}
