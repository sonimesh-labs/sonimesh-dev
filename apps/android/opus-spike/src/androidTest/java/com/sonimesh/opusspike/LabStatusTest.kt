package com.sonimesh.opusspike

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
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
                    LabStatus.publishEncoderRate(73)
                    requireNotNull(findButton(activity.window.decorView, "Copy lab evidence")).performClick()

                    val clip = requireNotNull(
                        activity.getSystemService(ClipboardManager::class.java).primaryClip,
                    )
                    val text = requireNotNull(clip.getItemAt(0).coerceToText(activity)).toString()
                    assertTrue(text.contains("Session request: Game, 128 kbps target"))
                    assertTrue(text.contains("Transport: ADB TCP"))
                    assertTrue(text.contains("Capture signal: no capturable audio"))
                    assertTrue(text.contains("Capture duration: 42s"))
                    assertTrue(text.contains("Sender queue drops: 7"))
                    assertTrue(text.contains("Media packets sent: 12"))
                    assertTrue(text.contains("Encoded payload average: 73 kbps"))
                } finally {
                    LabStatus.serviceFinished()
                    LabStatus.publishSenderQueueDrops(null)
                    LabStatus.publishSentPackets(null)
                    LabStatus.publishEncoderRate(null)
                    LabStatus.publishSignal(null)
                    LabStatus.publishCaptureDuration(null)
                }
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
    fun transportFlagsPreferPairedUdpAndDistinguishTheThreeCapturePaths() {
        assertEquals(LabTransport.Adb, LabTransport.fromFlags(udpMode = false, pairedMode = false))
        assertEquals(LabTransport.Udp, LabTransport.fromFlags(udpMode = true, pairedMode = false))
        assertEquals(LabTransport.PairedUdp, LabTransport.fromFlags(udpMode = true, pairedMode = true))
        assertEquals(LabTransport.PairedUdp, LabTransport.fromFlags(udpMode = false, pairedMode = true))
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
                assertTrue(hasStatus(activity.window.decorView, "Status: The lab invitation must be hexadecimal text."))
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
                    retryVisible = findButton(activity.window.decorView, "Retry last capture")?.visibility == View.VISIBLE
                }
                if (!retryVisible) SystemClock.sleep(50)
            }
            assertTrue("retry should appear after failed service cleanup", retryVisible)
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Status: Unable to start capture."))
                assertEquals(View.VISIBLE, findButton(activity.window.decorView, "Retry last capture")?.visibility)
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
