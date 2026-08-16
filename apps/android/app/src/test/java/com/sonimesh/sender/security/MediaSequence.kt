package com.sonimesh.sender.security

/**
 * Host-JVM mirror of `sonimesh-protocol`'s sender-owned `MediaSequence`.
 *
 * A fresh key epoch starts at zero and issues each sequence exactly once.
 * After `UInt.MAX_VALUE` the mirror refuses further sequences (returns null),
 * matching the Rust fail-closed exhaustion boundary that makes key rotation
 * mandatory before any media sequence number is reused.
 */
class MediaSequence private constructor(private var next: UInt?) {
    companion object {
        fun new(initial: UInt): MediaSequence = MediaSequence(initial)
    }

    fun takeNext(): UInt? {
        val sequence = next ?: return null
        next = if (sequence == UInt.MAX_VALUE) null else sequence + 1u
        return sequence
    }
}
