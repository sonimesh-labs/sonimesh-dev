package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
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

            runAllCaptureCleanupSteps(*steps)

            assertEquals((0 until stepCount).toList(), observed)
        }
    }
}
