package com.sonimesh.opusspike

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.core.app.ActivityScenario
import android.view.View
import android.view.ViewGroup
import android.widget.EditText
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LabAddressTest {
    @Test
    fun acceptsNumericUnicastAndRejectsUnsuitableDestinations() {
        assertEquals("10.0.2.2", parseLabAddress(" 10.0.2.2 ")?.hostAddress)
        assertEquals("192.168.1.7", parseLabAddress("192.168.1.7")?.hostAddress)
        for (invalid in listOf("", "desktop.local", "0.0.0.0", "127.0.0.1", "224.0.0.1", "255.255.255.255", "::1")) {
            assertNull(invalid, parseLabAddress(invalid))
        }
    }

    @Test
    fun addressFieldAcceptsAllIpv4SeparatorsAndSurvivesRecreation() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val field = requireNotNull(findField(activity.window.decorView))
                field.setText("192.168.1.7")
                assertEquals("192.168.1.7", field.text.toString())
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals("192.168.1.7", requireNotNull(findField(activity.window.decorView)).text.toString())
            }
        }
    }

    private fun findField(view: View): EditText? {
        if (view is EditText) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            findField(view.getChildAt(index))?.let { return it }
        }
        return null
    }
}
