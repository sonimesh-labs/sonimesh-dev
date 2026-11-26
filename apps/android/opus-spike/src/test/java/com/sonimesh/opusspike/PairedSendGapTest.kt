package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.ArrayBlockingQueue

class PairedSendGapTest {
    @Test
    fun queuedPacketRemainsEligibleForOldestDropWhileWaiting() {
        val queue = ArrayBlockingQueue<Int>(2)
        queue.add(1)
        queue.add(2)
        val packet = takePacedPacket(queue) {
            assertTrue(enqueueLatest(queue, 3))
            true
        }
        assertEquals(2, packet)
        assertEquals(3, queue.poll())
    }

    @Test
    fun cancelledWaitLeavesQueuedPacketUntouched() {
        val queue = ArrayBlockingQueue<Int>(2)
        queue.add(1)
        assertEquals(null, takePacedPacket(queue) { false })
        assertEquals(1, queue.poll())
    }

    @Test
    fun firstAndAlreadySpacedPacketsDoNotWait() {
        val sleep: (Long) -> Unit = { throw AssertionError("unexpected wait") }
        assertTrue(waitForPairedSendGap(null, { true }, { 0 }, sleep))
        assertTrue(waitForPairedSendGap(1_000_000, { true }, { 11_000_000 }, sleep))
    }

    @Test
    fun burstWaitsOnlyForTheRemainingGap() {
        var now = 3_000_000L
        val waits = mutableListOf<Long>()
        assertTrue(waitForPairedSendGap(0, { true }, { now }) {
            waits.add(it)
            now += it
        })
        assertEquals(listOf(7_000_000L), waits)
    }

    @Test
    fun stopOrInterruptCancelsTheWait() {
        assertFalse(waitForPairedSendGap(null, { false }, { 0 }) { throw AssertionError("unexpected wait") })
        var active = true
        assertFalse(waitForPairedSendGap(0, { active }, { 1 }) { active = false })
        try {
            assertFalse(waitForPairedSendGap(0, { true }, { 1 }) { throw InterruptedException() })
            assertTrue(Thread.currentThread().isInterrupted)
        } finally {
            Thread.interrupted()
        }
    }
}
