package com.sonimesh.opusspike

import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import androidx.test.core.app.ActivityScenario
import com.sonimesh.sender.protocol.PairingQr
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class LabPairingTest {
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
