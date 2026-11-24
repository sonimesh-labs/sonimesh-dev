package com.sonimesh.sender.pairing

import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class PairingSocketTest {
    @Test
    fun stalled_control_read_times_out_and_restores_socket_timeout() {
        ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { server ->
            Socket("127.0.0.1", server.localPort).use { client ->
                server.accept().use {
                    assertThrows(SocketTimeoutException::class.java) {
                        withControlReadTimeout(client, 100) { client.inputStream.read() }
                    }
                    assertEquals(0, client.soTimeout)
                    assertEquals(7, withControlReadTimeout(client, 100) {
                        assertEquals(100, client.soTimeout)
                        7
                    })
                    assertEquals(0, client.soTimeout)
                    assertThrows(IllegalArgumentException::class.java) {
                        withControlReadTimeout(client, 0) { }
                    }
                    assertThrows(IllegalArgumentException::class.java) {
                        withControlReadTimeout(client, 30_001) { }
                    }
                }
            }
        }
    }

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
