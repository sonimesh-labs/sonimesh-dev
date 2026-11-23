package com.sonimesh.opusspike

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class EncoderTailDrainTest {
    @Test
    fun drainsPendingOutputWhileWaitingToSubmitEnd() {
        var now = 0L
        var inputAttempts = 0
        val outputs = ArrayDeque(listOf(EncoderTailOutput.Progress, EncoderTailOutput.Empty,
            EncoderTailOutput.Progress, EncoderTailOutput.End))
        val events = mutableListOf<String>()
        val finished = drainEncoderTail(
            nowMillis = { now },
            submitEnd = {
                events.add("input")
                ++inputAttempts == 3
            },
            pollOutput = {
                outputs.removeFirst().also { events.add(it.name) }
            },
            idle = { now += 2 },
        )
        assertTrue(finished)
        assertEquals(listOf("input", "Progress", "input", "Empty", "input", "Progress", "End"), events)
    }

    @Test
    fun deadlineStopsWaitingEvenWithoutInputOrOutput() {
        var now = 0L
        var attempts = 0
        val finished = drainEncoderTail(
            nowMillis = { now },
            submitEnd = { attempts++; false },
            pollOutput = { EncoderTailOutput.Empty },
            idle = { now += 10 },
        )
        assertFalse(finished)
        assertEquals(250, now)
        assertEquals(25, attempts)
    }
}
