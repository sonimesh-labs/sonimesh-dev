package com.sonimesh.sender.security

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * JVM pin of the sender-side media sequence exhaustion boundary.
 *
 * The behavior mirrors the Rust pins in
 * `crates/sonimesh-protocol/src/media_key.rs` (`MediaSequence` and the
 * sealer-level `sealer_refuses_to_issue_a_sequence_beyond_u32_max` test), so
 * the Android implementation cannot drift from the mandatory key-rotation
 * guard that prevents sequence reuse within one media key epoch.
 */
class MediaSequenceVectorTest {
    @Test
    fun issuesSequencesInOrderFromANonZeroStart() {
        val sequence = MediaSequence.new(5u)
        assertEquals(5u, sequence.takeNext())
        assertEquals(6u, sequence.takeNext())
        assertEquals(7u, sequence.takeNext())
    }

    @Test
    fun refusesToReuseASequenceAfterMaxValue() {
        val sequence = MediaSequence.new(UInt.MAX_VALUE)
        assertEquals(UInt.MAX_VALUE, sequence.takeNext())
        assertNull(sequence.takeNext())
        assertNull(sequence.takeNext())
    }

    @Test
    fun freshEpochStartsAtZero() {
        val sequence = MediaSequence.new(0u)
        assertEquals(0u, sequence.takeNext())
        assertEquals(1u, sequence.takeNext())
    }
}
