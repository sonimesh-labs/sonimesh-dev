package com.sonimesh.opusspike

import android.view.View
import android.view.ViewGroup
import android.widget.Spinner
import androidx.test.core.app.ActivityScenario
import org.junit.Assert.assertEquals
import org.junit.Test

class LabCaptureUsageUiTest {
    @Test
    fun usageChoiceSurvivesActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val spinner = findUsage(activity.window.decorView)
                assertEquals(0, spinner.selectedItemPosition)
                spinner.setSelection(2)
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals(2, findUsage(activity.window.decorView).selectedItemPosition)
            }
        }
    }

    private fun findUsage(view: View): Spinner {
        if (view is Spinner && view.contentDescription == "Playback usage for capture") return view
        if (view is ViewGroup) {
            for (index in 0 until view.childCount) {
                val found = runCatching { findUsage(view.getChildAt(index)) }.getOrNull()
                if (found != null) return found
            }
        }
        error("capture usage selector not found")
    }
}
