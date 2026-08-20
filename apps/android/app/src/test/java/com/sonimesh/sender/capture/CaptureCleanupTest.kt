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
    fun a_single_setup_resource_release_failure_is_counted_without_being_rethrown() {
        val failedSteps = runAllCaptureCleanupSteps(
            { throw IllegalStateException("candidate record release failed") },
        )

        assertEquals(1, failedSteps)
    }

    @Test
    fun a_single_setup_resource_fatal_release_failure_is_rethrown() {
        val failure = AssertionError("candidate record release failed")

        val thrown = assertThrows(AssertionError::class.java) {
            runAllCaptureCleanupSteps({ throw failure })
        }

        assertSame(failure, thrown)
    }

    @Test
    fun collection_retains_fatal_failure_and_continues_after_it() {
        val fatal = AssertionError("stop request failed")
        val observed = mutableListOf<Int>()

        val result = collectCaptureCleanupFailures(
            {
                observed += 0
                throw IllegalStateException("ordinary failure")
            },
            {
                observed += 1
                throw fatal
            },
            {
                observed += 2
                throw SecurityException("later ordinary failure")
            },
        )

        assertEquals(listOf(0, 1, 2), observed)
        assertEquals(2, result.ordinaryFailureCount)
        assertSame(fatal, result.fatalFailure)
    }

    @Test
    fun merging_fatal_failures_preserves_the_first_and_suppresses_the_later() {
        val primary = AssertionError("setup failed")
        val secondary = LinkageError("candidate release failed")

        val merged = mergeCaptureFatalFailures(primary, secondary)

        assertSame(primary, merged)
        assertEquals(listOf(secondary), primary.suppressedExceptions)
    }

    @Test
    fun merging_without_a_primary_failure_adopts_the_cleanup_failure() {
        val secondary = LinkageError("candidate release failed")

        assertSame(secondary, mergeCaptureFatalFailures(null, secondary))
    }

    @Test
    fun deferred_fatal_failure_keeps_all_failures_until_finish_claimed() {
        val first = AssertionError("reader failed")
        val second = LinkageError("record stop failed")
        val deferred = DeferredCaptureFatalFailure()

        deferred.retain(first)
        deferred.retain(second)

        assertSame(first, deferred.take())
        assertEquals(listOf(second), first.suppressedExceptions)
        assertEquals(null, deferred.take())
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
