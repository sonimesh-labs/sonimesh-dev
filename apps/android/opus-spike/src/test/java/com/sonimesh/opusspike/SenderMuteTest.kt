package com.sonimesh.opusspike

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertSame
import org.junit.Test

class SenderMuteTest {
    @Test
    fun muteUsesSilenceWithoutDestroyingTheCapturedFrame() {
        val captured = shortArrayOf(1200, -1200, 300, -300)
        val silence = ShortArray(captured.size)

        assertSame(captured, encoderInput(captured, silence, false))
        assertArrayEquals(shortArrayOf(0, 0, 0, 0), encoderInput(captured, silence, true))
        assertSame(captured, encoderInput(captured, silence, false))
        assertArrayEquals(shortArrayOf(1200, -1200, 300, -300), captured)
    }
}
