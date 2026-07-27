package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Test

class CaptureStartAdmissionTest {
    @Test
    fun idle_generation_accepts_one_start() {
        assertEquals(
            CaptureStartAdmission.Start,
            captureStartAdmission(teardownStarted = false, ownsCaptureLifecycle = false),
        )
    }

    @Test
    fun active_generation_ignores_a_duplicate_start() {
        assertEquals(
            CaptureStartAdmission.IgnoreActiveDuplicate,
            captureStartAdmission(teardownStarted = false, ownsCaptureLifecycle = true),
        )
    }

    @Test
    fun retiring_generation_rejects_a_fresh_start_even_after_resources_are_retired() {
        for (ownsCaptureLifecycle in listOf(false, true)) {
            assertEquals(
                CaptureStartAdmission.RejectRetiringGeneration,
                captureStartAdmission(
                    teardownStarted = true,
                    ownsCaptureLifecycle = ownsCaptureLifecycle,
                ),
            )
        }
    }
}
