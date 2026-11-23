package com.sonimesh.opusspike

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.core.app.ActivityScenario
import android.view.View
import android.view.ViewGroup
import android.widget.EditText
import android.widget.Button
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

    @Test
    fun validAddressIsRememberedForNextActivityAndInvalidAddressIsNotSaved() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val preferences = activity.getPreferences(android.content.Context.MODE_PRIVATE)
                preferences.edit().clear().commit()
                val field = requireNotNull(findField(activity.window.decorView))
                val start = requireNotNull(findButton(activity.window.decorView, "Start UDP capture"))
                field.setText("invalid")
                start.performClick()
                assertNull(preferences.getString("desktopAddress", null))
                field.setText("192.168.1.7")
                start.performClick()
                assertEquals("192.168.1.7", preferences.getString("desktopAddress", null))
            }
        }
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                assertEquals("192.168.1.7", requireNotNull(findField(activity.window.decorView)).text.toString())
                activity.getPreferences(android.content.Context.MODE_PRIVATE).edit().clear().commit()
            }
        }
    }

    @Test
    fun udpAddressCanBeCopiedToControlWithoutChangingIndependentFields() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val udp = requireNotNull(findField(root))
                val control = requireNotNull(findField(root, "Paired control IPv4 address"))
                val copy = requireNotNull(findButton(root, "Use UDP address for control"))
                udp.setText("192.168.1.7")
                assertEquals("127.0.0.1", control.text.toString())
                copy.performClick()
                assertEquals("192.168.1.7", control.text.toString())
                udp.setText("192.168.1.8")
                assertEquals("192.168.1.7", control.text.toString())
                udp.setText("")
                copy.performClick()
                assertEquals("192.168.1.7", control.text.toString())
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals("192.168.1.7", requireNotNull(findField(activity.window.decorView, "Paired control IPv4 address")).text.toString())
            }
        }
    }

    @Test
    fun controlAddressCanBeCopiedToUdpButLoopbackCannot() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val udp = requireNotNull(findField(root))
                val control = requireNotNull(findField(root, "Paired control IPv4 address"))
                val copy = requireNotNull(findButton(root, "Use control address for UDP"))
                udp.setText("10.0.2.2")
                control.setText("127.0.0.1")
                copy.performClick()
                assertEquals("10.0.2.2", udp.text.toString())
                control.setText("192.168.1.7")
                copy.performClick()
                assertEquals("192.168.1.7", udp.text.toString())
                control.setText("192.168.1.8")
                assertEquals("192.168.1.7", udp.text.toString())
            }
        }
    }

    private fun findButton(view: View, text: String): Button? {
        if (view is Button && view.text == text) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            findButton(view.getChildAt(index), text)?.let { return it }
        }
        return null
    }

    private fun findField(view: View, description: String? = null): EditText? {
        if (view is EditText && (description == null || view.contentDescription == description)) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            findField(view.getChildAt(index), description)?.let { return it }
        }
        return null
    }
}
