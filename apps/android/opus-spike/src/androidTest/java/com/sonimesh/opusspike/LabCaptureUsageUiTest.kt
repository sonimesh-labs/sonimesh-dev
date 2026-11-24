package com.sonimesh.opusspike

import android.media.AudioAttributes
import android.graphics.Rect
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.Spinner
import android.widget.Switch
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import com.sonimesh.sender.encoder.OpusBitrate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LabCaptureUsageUiTest {
    @Test
    fun sessionControlsAreVisibleWithoutScrolling() {
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD) }
                androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().waitForIdleSync()
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    val stop = findButton(root, "Stop capture")
                    val mute = findMuteSwitch(root)
                    val tone = findButton(root, "Start test tone")
                    for (control in listOf(stop, mute, tone)) {
                        val visible = Rect()
                        assertTrue("${control.text} is not visible", control.getGlobalVisibleRect(visible))
                        assertEquals("${control.text} is clipped", control.height, visible.height())
                        assertEquals("${control.text} is clipped horizontally", control.width, visible.width())
                    }
                    assertTrue("mute should be available during capture", mute.isEnabled)
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.serviceFinished()
            }
        }
    }

    @Test
    fun testToneFailureLeavesControlReadyForRetry() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val tone = findButton(root, "Start test tone")
                tone.performClick()
                if (tone.text.toString() == "Start test tone") {
                    assertTrue(findStatus(root).text.toString().contains("Unable to play test tone"))
                } else {
                    findButton(root, "Stop test tone").performClick()
                }
            }
        }
    }

    @Test
    fun retryLastCaptureAppearsAfterConnectionFailureAndHidesForNewSession() {
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    assertEquals(View.GONE, findButton(activity.window.decorView, "Retry last capture").visibility)
                    LabStatus.publish("Desktop connection ended. Check the listener, then start capture again.")
                }
                androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().waitForIdleSync()
                scenario.onActivity { activity ->
                    assertEquals(View.VISIBLE, findButton(activity.window.decorView, "Retry last capture").visibility)
                    LabStatus.serviceStarted(LabCaptureUsage.All, OpusBitrate.STANDARD)
                    LabStatus.publish("Connecting to the desktop listener…")
                }
                androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().waitForIdleSync()
                scenario.onActivity { activity ->
                    assertEquals(View.GONE, findButton(activity.window.decorView, "Retry last capture").visibility)
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.serviceFinished()
            }
        }
    }

    @Test
    fun bitrateChoiceDefaultsTo96AndSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val spinner = findSpinner(activity.window.decorView, "Opus target bitrate for capture")
                assertEquals("96 kbps", spinner.selectedItem)
                spinner.setSelection(2)
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals("128 kbps", findSpinner(activity.window.decorView, "Opus target bitrate for capture").selectedItem)
            }
        }
    }

    @Test
    fun usageChoiceSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val spinner = findSpinner(activity.window.decorView, "Playback usage for capture")
                assertEquals(0, spinner.selectedItemPosition)
                spinner.setSelection(2)
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals(2, findSpinner(activity.window.decorView, "Playback usage for capture").selectedItemPosition)
            }
        }
    }

    @Test
    fun testToneUsesSelectedUsageAndRetainsItAcrossRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                findButton(root, "Start test tone").performClick()
                assertEquals(AudioAttributes.USAGE_MEDIA, activity.testToneUsage())
                findButton(root, "Stop test tone").performClick()
                findSpinner(root, "Playback usage for capture").setSelection(1)
                findButton(root, "Start test tone").performClick()
                assertEquals(AudioAttributes.USAGE_MEDIA, activity.testToneUsage())
                findButton(root, "Stop test tone").performClick()
                findSpinner(root, "Playback usage for capture").setSelection(2)
                findButton(root, "Start test tone").performClick()
                assertEquals(AudioAttributes.USAGE_GAME, activity.testToneUsage())
                findSpinner(root, "Playback usage for capture").setSelection(3)
                assertEquals(AudioAttributes.USAGE_GAME, activity.testToneUsage())
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                assertEquals(AudioAttributes.USAGE_GAME, activity.testToneUsage())
                findButton(root, "Stop test tone").performClick()
                findButton(root, "Start test tone").performClick()
                assertEquals(AudioAttributes.USAGE_UNKNOWN, activity.testToneUsage())
                findButton(root, "Stop test tone").performClick()
            }
        }
    }

    @Test
    fun newToneUsesCaptureUsageUntilServiceFinishes() {
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    LabStatus.serviceStarted(LabCaptureUsage.Media, OpusBitrate.STANDARD)
                    val root = activity.window.decorView
                    findSpinner(root, "Playback usage for capture").setSelection(2)
                    findButton(root, "Start test tone").performClick()
                    assertEquals(AudioAttributes.USAGE_MEDIA, activity.testToneUsage())
                    findButton(root, "Stop test tone").performClick()
                }
                scenario.recreate()
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    findButton(root, "Start test tone").performClick()
                    assertEquals(AudioAttributes.USAGE_MEDIA, activity.testToneUsage())
                    findButton(root, "Stop test tone").performClick()
                    LabStatus.serviceFinished()
                    findButton(root, "Start test tone").performClick()
                    assertEquals(AudioAttributes.USAGE_GAME, activity.testToneUsage())
                    findButton(root, "Stop test tone").performClick()
                }
            }
        } finally {
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().runOnMainSync {
                LabStatus.serviceFinished()
            }
        }
    }

    private fun findButton(view: View, text: String): Button {
        if (view is Button && view.text.toString() == text) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                val found = runCatching { findButton(view.getChildAt(index), text) }.getOrNull()
                if (found != null) return found
            }
        }
        error("button not found: $text")
    }

    private fun findMuteSwitch(view: View): Switch {
        if (view is Switch && view.text.toString() == "Mute sending") return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                val found = runCatching { findMuteSwitch(view.getChildAt(index)) }.getOrNull()
                if (found != null) return found
            }
        }
        error("mute switch not found")
    }

    private fun findStatus(view: View): TextView {
        if (view is TextView && view.text.toString().startsWith("Status:")) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                val found = runCatching { findStatus(view.getChildAt(index)) }.getOrNull()
                if (found != null) return found
            }
        }
        error("status not found")
    }

    private fun findSpinner(view: View, description: String): Spinner {
        if (view is Spinner && view.contentDescription == description) return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                val found = runCatching { findSpinner(view.getChildAt(index), description) }.getOrNull()
                if (found != null) return found
            }
        }
        error("selector not found: $description")
    }
}
