package com.sonimesh.opusspike

import android.media.AudioAttributes
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.Spinner
import androidx.test.core.app.ActivityScenario
import org.junit.Assert.assertEquals
import org.junit.Test

class LabCaptureUsageUiTest {
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
