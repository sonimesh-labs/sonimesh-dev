package com.sonimesh.opusspike

import android.app.Activity
import android.app.Instrumentation
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import com.sonimesh.sender.protocol.PairingQr
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class LabPairingTest {
    @Test
    fun direct_invitation_can_be_pasted_and_shared() {
        val payload = "SONIMESH-LAB1:39094:${labHex(labInvitation("AB".repeat(32)))}"
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                activity.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("invitation", payload))
                requireNotNull(findButton(root, "Paste lab invitation")).performClick()
                assertEquals(payload, requireNotNull(findField(root, "Lab invitation payload")).text.toString())
                requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                assertEquals("39094", requireNotNull(findField(root, "Desktop UDP port")).text.toString())
            }
        }
        val intent = Intent(InstrumentationRegistry.getInstrumentation().targetContext, LabActivity::class.java).apply {
            action = Intent.ACTION_SEND
            type = "text/plain"
            putExtra(Intent.EXTRA_TEXT, payload)
        }
        ActivityScenario.launch<LabActivity>(intent).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                assertEquals(payload, requireNotNull(findField(root, "Lab invitation payload")).text.toString())
                requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                assertEquals("39094", requireNotNull(findField(root, "Desktop UDP port")).text.toString())
            }
        }
    }

    @Test
    fun direct_invitation_applies_port_only_after_payload_validation() {
        val payload = labHex(labInvitation("AB".repeat(32)))
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val port = requireNotNull(findField(root, "Desktop UDP port"))
                val host = requireNotNull(findField(root, "Paired control IPv4 address"))
                port.setText("39093")
                activity.applyScannedInvitation("SONIMESH-LAB1:39094:$payload")
                assertEquals("39094", port.text.toString())
                assertEquals("127.0.0.1", host.text.toString())
                activity.applyScannedInvitation(payload)
                assertEquals("39094", port.text.toString())
                for (invalid in listOf("SONIMESH-LAB1:0:$payload", "SONIMESH-LAB1:65536:$payload", "SONIMESH-LAB1:39095:00", "SONIMESH-LAB2:39095:$payload")) {
                    activity.applyScannedInvitation(invalid)
                    assertEquals("39094", port.text.toString())
                    assertEquals("127.0.0.1", host.text.toString())
                }
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals("39094", requireNotNull(findField(activity.window.decorView, "Desktop UDP port")).text.toString())
            }
        }
    }

    @Test
    fun test_tone_continues_across_activity_recreation_and_can_be_stopped() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                requireNotNull(findButton(activity.window.decorView, "Start test tone")).performClick()
                assertTrue(activity.isTestTonePlaying())
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertTrue(activity.isTestTonePlaying())
                requireNotNull(findButton(activity.window.decorView, "Stop test tone")).performClick()
                assertFalse(activity.isTestTonePlaying())
                requireNotNull(findButton(activity.window.decorView, "Start test tone"))
            }
        }
    }

    @Test
    fun scan_button_launches_embedded_scanner_and_applies_its_result() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val result = Intent().putExtra("SCAN_RESULT", labHex(labInvitation("AB".repeat(32), "192.168.1.7")).uppercase())
        val monitor = instrumentation.addMonitor(
            "com.journeyapps.barcodescanner.CaptureActivity",
            Instrumentation.ActivityResult(Activity.RESULT_OK, result),
            true,
        )
        try {
            ActivityScenario.launch(LabActivity::class.java).use { scenario ->
                scenario.onActivity { activity ->
                    requireNotNull(findButton(activity.window.decorView, "Scan lab invitation")).performClick()
                }
                instrumentation.waitForIdleSync()
                assertEquals(1, monitor.hits)
                scenario.onActivity { activity ->
                    val root = activity.window.decorView
                    assertEquals("192.168.1.7", requireNotNull(findField(root, "Paired control IPv4 address")).text.toString())
                    assertEquals("192.168.1.7", requireNotNull(findField(root, "Desktop IPv4 address for UDP")).text.toString())
                    assertEquals("ab".repeat(32), requireNotNull(findField(root, "Paired lab desktop SPKI")).text.toString())
                }
            }
        } finally {
            instrumentation.removeMonitor(monitor)
        }
    }

    @Test
    fun accepts_hex_pin_and_rejects_invalid_input() {
        val invitation = PairingQr.decode(labInvitation(" ${"AB".repeat(32)} "))
        assertEquals("127.0.0.1", invitation.controlHost)
        assertEquals(47610, invitation.controlPort)
        assertEquals(32, invitation.desktopTlsSpki.size)
        assertEquals("192.168.1.7", PairingQr.decode(labInvitation("AB".repeat(32), "192.168.1.7")).controlHost)
        assertThrows(IllegalArgumentException::class.java) { labInvitation("0".repeat(63)) }
        assertThrows(IllegalArgumentException::class.java) { labInvitation("GG".repeat(32)) }
        assertThrows(IllegalArgumentException::class.java) { labInvitation("aa".repeat(513)) }
        assertEquals("00ff", labHex(byteArrayOf(0, 0xff.toByte())))
        assertThrows(IllegalArgumentException::class.java) { labHexToBytes("") }
        assertThrows(IllegalArgumentException::class.java) { labHexToBytes("0") }
        assertThrows(IllegalArgumentException::class.java) { labHexToBytes("gg") }
        assertThrows(IllegalArgumentException::class.java) {
            labHexToBytes("00".repeat(PairingQr.MAX_PAYLOAD_BYTES + 1))
        }
    }

    @Test
    fun invitation_payload_fills_control_fields_without_overwriting_udp_address() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val udp = requireNotNull(findField(root, "Desktop IPv4 address for UDP"))
                val invitation = requireNotNull(findField(root, "Lab invitation payload"))
                val pin = requireNotNull(findField(root, "Paired lab desktop SPKI"))
                val host = requireNotNull(findField(root, "Paired control IPv4 address"))
                val apply = requireNotNull(findButton(root, "Apply invitation payload"))
                udp.setText("10.0.2.2")
                invitation.setText(labHex(labInvitation("AB".repeat(32), "192.168.1.7")))
                apply.performClick()
                assertEquals("10.0.2.2", udp.text.toString())
                assertEquals("ab".repeat(32), pin.text.toString())
                assertEquals("192.168.1.7", host.text.toString())
                assertEquals("", invitation.text.toString())
            }
        }
    }

    @Test
    fun unapplied_invitation_survives_recreation_and_can_be_applied() {
        val payload = labHex(labInvitation("AB".repeat(32), "192.168.1.7"))
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                requireNotNull(findField(activity.window.decorView, "Lab invitation payload")).setText(payload)
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val invitation = requireNotNull(findField(root, "Lab invitation payload"))
                assertEquals(payload, invitation.text.toString())
                requireNotNull(findButton(root, "Apply invitation payload")).performClick()
                assertEquals("192.168.1.7", requireNotNull(findField(root, "Paired control IPv4 address")).text.toString())
                assertEquals("ab".repeat(32), requireNotNull(findField(root, "Paired lab desktop SPKI")).text.toString())
                assertEquals("", invitation.text.toString())
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                assertEquals("", requireNotNull(findField(activity.window.decorView, "Lab invitation payload")).text.toString())
            }
        }
    }

    @Test
    fun invalid_pasted_invitation_does_not_start_capture() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val invitation = requireNotNull(findField(root, "Lab invitation payload"))
                invitation.setText("invalid")
                requireNotNull(findButton(root, "Apply invitation and start paired capture")).performClick()
                assertEquals("invalid", invitation.text.toString())
                assertTrue(LabStatus.captureAdmission.canBegin())
            }
        }
    }

    @Test
    fun invitation_can_fill_udp_address_without_replacing_it_with_loopback() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val udp = requireNotNull(findField(root, "Desktop IPv4 address for UDP"))
                val invitation = requireNotNull(findField(root, "Lab invitation payload"))
                val apply = requireNotNull(findButton(root, "Apply invitation and use address for UDP"))
                udp.setText("10.0.2.2")
                invitation.setText(labHex(labInvitation("AB".repeat(32), "192.168.1.7")))
                apply.performClick()
                assertEquals("192.168.1.7", udp.text.toString())
                udp.setText("10.0.2.2")
                invitation.setText(labHex(labInvitation("AB".repeat(32))))
                apply.performClick()
                assertEquals("10.0.2.2", udp.text.toString())
                invitation.setText("invalid")
                apply.performClick()
                assertEquals("10.0.2.2", udp.text.toString())
            }
        }
    }

    @Test
    fun scanned_lab_invitation_updates_fields_and_keeps_loopback_udp_separate() {
        ActivityScenario.launch(LabActivity::class.java).use { scenario ->
            scenario.onActivity { activity ->
                val root = activity.window.decorView
                val udp = requireNotNull(findField(root, "Desktop IPv4 address for UDP"))
                val pin = requireNotNull(findField(root, "Paired lab desktop SPKI"))
                val host = requireNotNull(findField(root, "Paired control IPv4 address"))
                activity.applyScannedInvitation(labHex(labInvitation("AB".repeat(32), "192.168.1.7")).uppercase())
                assertEquals("192.168.1.7", udp.text.toString())
                assertEquals("192.168.1.7", host.text.toString())
                assertEquals("ab".repeat(32), pin.text.toString())
                udp.setText("10.0.2.2")
                activity.applyScannedInvitation(labHex(labInvitation("CD".repeat(32))))
                assertEquals("10.0.2.2", udp.text.toString())
                assertEquals("127.0.0.1", host.text.toString())
                activity.applyScannedInvitation("not an invitation")
                assertEquals("127.0.0.1", host.text.toString())
                assertEquals("cd".repeat(32), pin.text.toString())
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

    private fun findField(view: View, description: String): EditText? {
        if (view is EditText && view.contentDescription == description) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) {
            findField(view.getChildAt(index), description)?.let { return it }
        }
        return null
    }
}
