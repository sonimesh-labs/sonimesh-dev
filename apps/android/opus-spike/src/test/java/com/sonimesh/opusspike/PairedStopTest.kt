package com.sonimesh.opusspike

import com.sonimesh.sender.pairing.StreamControlResult
import com.sonimesh.sender.pairing.StreamIdentity
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
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

    @Test fun tailFailureIsReportedAlongsideAnUnconfirmedStop() {
        val confirmed = Result.success<StreamControlResult>(StreamControlResult.Status(identity, 4, ""))
        val failed = Result.failure<StreamControlResult>(java.io.IOException("disconnected"))
        assertEquals("Capture stopped, but the end of the audio may be missing.", captureStopWarning(false, confirmed, true))
        assertEquals("Capture stopped, but the end of the audio may be missing.", captureStopWarning(false, null, true))
        val warning = requireNotNull(captureStopWarning(false, failed, true))
        assertTrue(warning.contains("end of the audio may be missing"))
        assertTrue(warning.contains("desktop did not confirm"))
        assertNull(captureStopWarning(true, failed, true))
    }
}
