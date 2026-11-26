package com.sonimesh.opusspike

import org.junit.Assert.assertArrayEquals
import org.junit.Test

class SenderMuteTest {
    @Test
    fun muteUsesSilenceWithoutDestroyingTheCapturedFrame() {
        val captured = shortArrayOf(1200, -1200, 300, -300)
        val processor = SenderGainProcessor(captured.size)

        assertArrayEquals(captured, processor.prepare(captured, false, 100))
        assertArrayEquals(shortArrayOf(0, 0, 0, 0), processor.prepare(captured, true, 200))
        processor.accepted()
        assertArrayEquals(shortArrayOf(0, 0, 300, -300), processor.prepare(captured, false, 100))
        processor.accepted()
        assertArrayEquals(captured, processor.prepare(captured, false, 100))
        assertArrayEquals(shortArrayOf(1200, -1200, 300, -300), captured)
    }
}
