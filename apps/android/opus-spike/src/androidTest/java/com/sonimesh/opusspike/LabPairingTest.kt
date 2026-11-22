package com.sonimesh.opusspike

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
    }
}
