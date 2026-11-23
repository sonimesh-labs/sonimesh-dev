package com.sonimesh.opusspike

import android.app.Activity
import android.content.Intent
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.sonimesh.sender.encoder.OpusBitrate
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
                assertTrue(findButton(activity.window.decorView, "Start test tone") != null)
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
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Status: Unable to start capture."))
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

    private fun hasStatus(view: View, prefix: String): Boolean {
        if (view is TextView && view.text.toString().startsWith(prefix)) return true
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                if (hasStatus(view.getChildAt(index), prefix)) return true
            }
        }
        return false
    }
}
