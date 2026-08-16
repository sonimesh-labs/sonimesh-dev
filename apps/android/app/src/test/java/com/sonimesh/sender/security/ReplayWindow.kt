package com.sonimesh.sender.security

/** Width of the authenticated replay window in sequence numbers. */
val REPLAY_WINDOW_BITS = 64u

/** Result of recording an already-authenticated sequence number. */
enum class ReplayDecision {
    /** The sequence has not appeared in the active window. */
    ACCEPTED,

    /** The sequence is already present in the active window. */
    DUPLICATE,

    /** The sequence precedes the active replay window. */
    TOO_OLD,
}

/**
 * Host-JVM mirror of `sonimesh-protocol`'s fixed-size `ReplayWindow`.
 *
 * Call [acceptAuthenticated] only after AEAD authentication succeeds; replay
 * state advances only for authenticated packets. A sequence wrap requires a
 * new key epoch and a fresh window, mirroring the Rust replay guard.
 */
class ReplayWindow {
    private var highest: UInt? = null
    private var seen = 0uL

    /**
     * Records one already-authenticated sequence and returns whether it is
     * new, a duplicate, or precedes the active replay window.
     */
    fun acceptAuthenticated(sequence: UInt): ReplayDecision {
        val highestValue = highest
        if (highestValue == null) {
            highest = sequence
            seen = 1uL
            return ReplayDecision.ACCEPTED
        }

        if (sequence > highestValue) {
            val shift = sequence - highestValue
            seen = if (shift >= REPLAY_WINDOW_BITS) {
                1uL
            } else {
                (seen shl shift.toInt()) or 1uL
            }
            highest = sequence
            return ReplayDecision.ACCEPTED
        }

        val distance = highestValue - sequence
        if (distance >= REPLAY_WINDOW_BITS) {
            return ReplayDecision.TOO_OLD
        }
        val bit = 1uL shl distance.toInt()
        return if (seen and bit != 0uL) {
            ReplayDecision.DUPLICATE
        } else {
            seen = seen or bit
            ReplayDecision.ACCEPTED
        }
    }
}
