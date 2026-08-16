package com.sonimesh.sender.security

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * JVM pin of the receiver-side authenticated replay window.
 *
 * The behavior mirrors the Rust pins in
 * `crates/sonimesh-protocol/src/replay.rs` (`ReplayWindow` unit tests), so the
 * Android replay guard cannot drift from the Rust duplicate-detection and
 * window-boundary semantics.
 */
class ReplayWindowVectorTest {
    @Test
    fun acceptsReorderingOnce() {
        val window = ReplayWindow()
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(10u))
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(12u))
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(11u))
        assertEquals(ReplayDecision.DUPLICATE, window.acceptAuthenticated(11u))
    }

    @Test
    fun rejectsValuesOutsideWindow() {
        val window = ReplayWindow()
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(1u))
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(65u))
        assertEquals(ReplayDecision.TOO_OLD, window.acceptAuthenticated(1u))
    }

    @Test
    fun largeJumpResetsBitmapWithoutAllocating() {
        val window = ReplayWindow()
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(2u))
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(1000u))
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(999u))
    }

    @Test
    fun keepsTheOldestSequenceInWindowAfterAMaximumInWindowShift() {
        val window = ReplayWindow()
        assertEquals(ReplayDecision.ACCEPTED, window.acceptAuthenticated(1u))
        assertEquals(
            ReplayDecision.ACCEPTED,
            window.acceptAuthenticated(1u + (REPLAY_WINDOW_BITS - 1u)),
        )
        // The previous highest is still inside the window (distance 63).
        assertEquals(ReplayDecision.DUPLICATE, window.acceptAuthenticated(1u))
        // One more advance pushes it out (distance 64).
        assertEquals(
            ReplayDecision.ACCEPTED,
            window.acceptAuthenticated(1u + REPLAY_WINDOW_BITS),
        )
        assertEquals(ReplayDecision.TOO_OLD, window.acceptAuthenticated(1u))
    }

    @Test
    fun acceptsTheOldestInWindowSequenceAtTheMaximumValue() {
        val window = ReplayWindow()
        assertEquals(
            ReplayDecision.ACCEPTED,
            window.acceptAuthenticated(UInt.MAX_VALUE),
        )
        assertEquals(
            ReplayDecision.ACCEPTED,
            window.acceptAuthenticated(UInt.MAX_VALUE - (REPLAY_WINDOW_BITS - 1u)),
        )
        assertEquals(
            ReplayDecision.TOO_OLD,
            window.acceptAuthenticated(UInt.MAX_VALUE - REPLAY_WINDOW_BITS),
        )
        assertEquals(
            ReplayDecision.DUPLICATE,
            window.acceptAuthenticated(UInt.MAX_VALUE - (REPLAY_WINDOW_BITS - 1u)),
        )
    }
}
