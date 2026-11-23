package com.sonimesh.opusspike

import org.junit.Assert.assertThrows
import org.junit.Test

class CaptureReadWaitTest {
    @Test
    fun stopsOnlyAfterConsecutiveEmptyReadsReachTheDeadline() {
        val wait = CaptureReadWait()
        wait.observe(0, 100)
        wait.observe(0, 5_099)
        assertThrows(CaptureReadStalledException::class.java) { wait.observe(0, 5_100) }
    }

    @Test
    fun positiveReadsAndClockRegressionStartANewWait() {
        val wait = CaptureReadWait()
        wait.observe(0, 100)
        wait.observe(2, 5_000)
        wait.observe(0, 5_100)
        wait.observe(0, 10)
        wait.observe(0, 5_009)
        assertThrows(CaptureReadStalledException::class.java) { wait.observe(0, 5_010) }
    }
}
