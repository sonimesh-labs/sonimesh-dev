package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Test

class CaptureCleanupTest {
    @Test
    fun every_cleanup_step_runs_once_when_any_individual_step_fails() {
        val stepCount = 6

        repeat(stepCount) { failingStep ->
            val observed = mutableListOf<Int>()
            val steps = Array<() -> Unit>(stepCount) { step ->
                {
                    observed += step
                    if (step == failingStep) {
                        throw IllegalStateException("injected cleanup failure")
                    }
                }
            }

            val failedSteps = runAllCaptureCleanupSteps(*steps)

            assertEquals(1, failedSteps)
            assertEquals((0 until stepCount).toList(), observed)
        }
    }


    @Test
    fun ordinary_failure_count_is_exact() {
        val failedSteps = runAllCaptureCleanupSteps(
            { throw IllegalStateException("first") },
            {},
            { throw SecurityException("second") },
        )

        assertEquals(2, failedSteps)
    }

    @Test
    fun fatal_failure_is_rethrown_after_every_cleanup_step_is_attempted() {
        val stepCount = 6

        repeat(stepCount) { failingStep ->
            val observed = mutableListOf<Int>()
            val failure = AssertionError("injected fatal cleanup failure")
            val steps = Array<() -> Unit>(stepCount) { step ->
                {
                    observed += step
                    if (step == failingStep) {
                        throw failure
                    }
                }
            }

            val thrown = assertThrows(AssertionError::class.java) {
                runAllCaptureCleanupSteps(*steps)
            }

            assertSame(failure, thrown)
            assertEquals((0 until stepCount).toList(), observed)
        }
    }
}
