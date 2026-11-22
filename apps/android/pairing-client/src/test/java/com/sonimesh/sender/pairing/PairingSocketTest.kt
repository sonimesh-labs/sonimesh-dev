package com.sonimesh.sender.pairing

import java.net.Socket
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingSocketTest {
    @Test
    fun invalidInvitationClosesCallerSuppliedSocket() {
        val socket = Socket()
        try {
            PairingClient.connect(byteArrayOf(), socket)
            throw AssertionError("invalid invitation accepted")
        } catch (_: IllegalArgumentException) {
            assertTrue(socket.isClosed)
        }
    }
}
