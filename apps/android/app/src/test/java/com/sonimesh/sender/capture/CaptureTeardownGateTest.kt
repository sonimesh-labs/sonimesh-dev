package com.sonimesh.sender.capture

import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureTeardownGateTest {
    @Test
    fun completion_waits_for_teardown_and_reader_exit() {
        val gate = CaptureTeardownGate()

        assertFalse(gate.claimFinish())
        assertTrue(gate.beginTeardown())
        gate.markReaderRunning()
        assertFalse(gate.claimFinish())

        gate.markReaderExited()
        assertTrue(gate.claimFinish())
    }

    @Test
    fun teardown_and_completion_each_have_one_winner() {
        val gate = CaptureTeardownGate()

        assertTrue(gate.beginTeardown())
        assertFalse(gate.beginTeardown())
        assertTrue(gate.claimFinish())
        assertFalse(gate.claimFinish())
    }

    @Test
    fun concurrent_finish_claims_have_exactly_one_winner() {
        val gate = CaptureTeardownGate()
        val contenders = 8
        val start = CountDownLatch(1)
        val completed = CountDownLatch(contenders)
        val winners = AtomicInteger()

        assertTrue(gate.beginTeardown())
        repeat(contenders) {
            Thread {
                start.await()
                if (gate.claimFinish()) {
                    winners.incrementAndGet()
                }
                completed.countDown()
            }.start()
        }
        start.countDown()

        assertTrue(completed.await(2, TimeUnit.SECONDS))
        assertEquals(1, winners.get())
    }
}
