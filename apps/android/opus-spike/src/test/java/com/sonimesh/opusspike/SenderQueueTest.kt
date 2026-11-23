package com.sonimesh.opusspike

import java.util.concurrent.ArrayBlockingQueue
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SenderQueueTest {
    @Test
    fun keepsRecentPacketsAndReportsEachDiscard() {
        val queue = ArrayBlockingQueue<Int>(2)
        assertFalse(enqueueLatest(queue, 1))
        assertFalse(enqueueLatest(queue, 2))
        assertTrue(enqueueLatest(queue, 3))
        assertTrue(enqueueLatest(queue, 4))
        assertEquals(3, queue.poll())
        assertEquals(4, queue.poll())
        assertFalse(enqueueLatest(queue, 5))
        assertEquals(5, queue.poll())
    }

    @Test
    fun sentPacketCounterSaturatesAtLongMaximum() {
        assertEquals(1L, incrementSaturated(0))
        assertEquals(Long.MAX_VALUE, incrementSaturated(Long.MAX_VALUE - 1))
        assertEquals(Long.MAX_VALUE, incrementSaturated(Long.MAX_VALUE))
    }
}
