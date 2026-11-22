package com.sonimesh.opusspike

import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SetupSocketTest {
    @Test
    fun cancellationClosesSocketAndInterruptsPendingRead() {
        ServerSocket(0).use { listener ->
            Socket("127.0.0.1", listener.localPort).use { client ->
                listener.accept().use {
                    val setup = SetupSocket()
                    assertTrue(setup.track(client))
                    val worker = Executors.newSingleThreadExecutor()
                    try {
                        val enteredRead = CountDownLatch(1)
                        val read = worker.submit<Boolean> {
                            enteredRead.countDown()
                            try { client.getInputStream().read(); false } catch (_: java.io.IOException) { true }
                        }
                        assertTrue(enteredRead.await(2, TimeUnit.SECONDS))
                        try {
                            read.get(100, TimeUnit.MILLISECONDS)
                            throw AssertionError("read unexpectedly completed before cancellation")
                        } catch (_: TimeoutException) { }
                        setup.cancel()
                        assertTrue(read.get(2, TimeUnit.SECONDS))
                        assertFalse(setup.handoff())
                    } finally {
                        worker.shutdownNow()
                    }
                }
            }
        }
    }

    @Test
    fun cancellationClosesLateReplacementButNotHandedOffSocket() {
        val cancelled = SetupSocket()
        val initial = Socket()
        assertTrue(cancelled.track(initial))
        cancelled.cancel()
        val late = Socket()
        assertFalse(cancelled.track(late))
        assertTrue(initial.isClosed)
        assertTrue(late.isClosed)

        val active = SetupSocket()
        val socket = Socket()
        assertTrue(active.track(socket))
        assertTrue(active.handoff())
        active.cancel()
        assertFalse(socket.isClosed)
        socket.close()
    }
}
