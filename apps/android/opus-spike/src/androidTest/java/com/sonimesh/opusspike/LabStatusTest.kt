package com.sonimesh.opusspike

import android.content.Intent
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LabStatusTest {
    @Test
    fun senderQueueDropsSurviveActivityRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                LabStatus.publishSenderQueueDrops(7)
                LabStatus.publishEncoderRate(73)
                assertTrue(hasStatus(activity.window.decorView, "Sender queue drops: 7"))
                assertTrue(hasStatus(activity.window.decorView, "Encoded payload average: 73 kbps"))
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(hasStatus(activity.window.decorView, "Sender queue drops: 7"))
                assertTrue(hasStatus(activity.window.decorView, "Encoded payload average: 73 kbps"))
                LabStatus.publishSenderQueueDrops(null)
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
