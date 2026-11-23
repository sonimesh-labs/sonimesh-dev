package com.sonimesh.opusspike

import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class EncoderInputWaitTest {
    @Test
    fun waitsWithoutSpinningAndStopsIfTheSameFrameRemainsBlocked() {
        val wait = EncoderInputWait()
        assertTrue(wait.shouldIdle(100, EncoderTailOutput.Empty))
        assertFalse(wait.shouldIdle(1_000, EncoderTailOutput.Progress))
        assertTrue(wait.shouldIdle(2_099, EncoderTailOutput.Empty))
        assertThrows(EncoderStalledException::class.java) { wait.shouldIdle(2_100, EncoderTailOutput.Empty) }
    }

    @Test
    fun acceptingAFrameStartsAFreshWaitForTheNextFrame() {
        val wait = EncoderInputWait()
        wait.shouldIdle(10, EncoderTailOutput.Empty)
        wait.reset()
        assertTrue(wait.shouldIdle(2_010, EncoderTailOutput.Empty))
        assertTrue(wait.shouldIdle(4_009, EncoderTailOutput.Empty))
        assertThrows(EncoderStalledException::class.java) { wait.shouldIdle(4_010, EncoderTailOutput.Empty) }
    }
}
