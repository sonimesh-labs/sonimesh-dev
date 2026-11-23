package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertEquals
import org.junit.Test

class PairedStopTest {
    private val identity = StreamIdentity(1, 1, 0)

    @Test fun confirmedStopNeedsNoWarning() {
        val result = attemptPairedStop(identity, false) { StreamControlResult.Status(it, 4, "") }
        assertNull(captureStopWarning(false, result))
    }

    @Test fun rejectedUnexpectedAndFailedStopsWarn() {
        val results = listOf(
            attemptPairedStop(identity, false) { StreamControlResult.Rejected(1, false, "") },
            attemptPairedStop(identity, false) { StreamControlResult.Status(it, 2, "") },
            attemptPairedStop(identity, false) { throw java.io.IOException("disconnected") },
        )
        results.forEach { assertNotNull(captureStopWarning(false, it)) }
        results.forEach { assertNull(captureStopWarning(true, it)) }
    }

    @Test fun unstartedOrPreviouslyFailedControlIsNotStoppedAgain() {
        var calls = 0
        val stop: (StreamIdentity) -> StreamControlResult = {
            calls++
            StreamControlResult.Status(it, 4, "")
        }
        assertNull(attemptPairedStop(null, false, stop))
        assertNull(attemptPairedStop(identity, true, stop))
        assertEquals(0, calls)
        assertNull(captureStopWarning(false, null))
    }
}
