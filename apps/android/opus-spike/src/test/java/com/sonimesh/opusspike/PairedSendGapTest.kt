package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairedSendGapTest {
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
