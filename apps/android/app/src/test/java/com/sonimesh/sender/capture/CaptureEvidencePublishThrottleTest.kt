package com.sonimesh.sender.capture

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptureEvidencePublishThrottleTest {
    @Test
    fun first_publication_is_allowed_for_any_non_negative_timestamp() {
        val early = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(early.shouldPublish(nowMillis = 0))

        val late = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(late.shouldPublish(nowMillis = 9_999_999))
    }

    @Test
    fun publications_within_interval_are_suppressed_until_boundary() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(throttle.shouldPublish(nowMillis = 100))

        assertFalse(throttle.shouldPublish(nowMillis = 109))
        assertFalse(throttle.shouldPublish(nowMillis = 1_099))
        assertTrue(throttle.shouldPublish(nowMillis = 1_100))
    }

    @Test
    fun publication_at_exact_interval_boundary_is_allowed() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(throttle.shouldPublish(nowMillis = 0))

        assertTrue(throttle.shouldPublish(nowMillis = 1_000))
        assertTrue(throttle.shouldPublish(nowMillis = 2_000))
    }

    @Test
    fun clock_regression_publishes_once_then_rebases_the_window() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(throttle.shouldPublish(nowMillis = 5_000))
        assertFalse(throttle.shouldPublish(nowMillis = 5_500))

        // A nonnegative clock regression rebases instead of suppressing every
        // publication until the previous timestamp is reached again.
        assertTrue(throttle.shouldPublish(nowMillis = 4_000))
        assertFalse(throttle.shouldPublish(nowMillis = 4_500))
        assertTrue(throttle.shouldPublish(nowMillis = 5_000))
    }

    @Test
    fun constructor_rejects_non_positive_interval() {
        assertThrows(IllegalArgumentException::class.java) {
            CaptureEvidencePublishThrottle(minimumIntervalMillis = 0)
        }
        assertThrows(IllegalArgumentException::class.java) {
            CaptureEvidencePublishThrottle(minimumIntervalMillis = -1)
        }
    }

    @Test
    fun negative_publication_time_is_rejected() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)

        assertThrows(IllegalArgumentException::class.java) {
            throttle.shouldPublish(nowMillis = -1)
        }
    }

    @Test
    fun allowed_publication_updates_the_reference_timestamp() {
        val throttle = CaptureEvidencePublishThrottle(minimumIntervalMillis = 1_000)
        assertTrue(throttle.shouldPublish(nowMillis = 0))

        // Only successful publications advance the reference timestamp.
        assertFalse(throttle.shouldPublish(nowMillis = 500))
        assertTrue(throttle.shouldPublish(nowMillis = 1_000))
        assertFalse(throttle.shouldPublish(nowMillis = 1_500))
    }
}
