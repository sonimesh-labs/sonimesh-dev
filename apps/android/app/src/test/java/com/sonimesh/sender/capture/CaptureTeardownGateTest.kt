package com.sonimesh.sender.capture

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
}
