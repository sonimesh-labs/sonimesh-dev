package com.sonimesh.sender.capture

import java.util.concurrent.atomic.AtomicInteger

/** Coordinates one capture teardown without making resource ownership concurrent. */
class CaptureTeardownGate {
    private val state = AtomicInteger(0)

    fun beginTeardown(): Boolean = setFlagUnlessPresent(TEARDOWN_STARTED)

    fun isTeardownStarted(): Boolean = state.get() and TEARDOWN_STARTED != 0

    /**
     * Marks one reader as running before invoking its start operation.
     *
     * Returns false without invoking [start] once teardown has begun. If [start]
     * throws, the running mark is rolled back before the exception is rethrown.
     */
    fun launchReader(start: () -> Unit): Boolean {
        while (true) {
            val current = state.get()
            if (current and TEARDOWN_STARTED != 0) {
                return false
            }
            check(current and READER_RUNNING == 0) { "capture reader is already running" }
            if (state.compareAndSet(current, current or READER_RUNNING)) {
                break
            }
        }

        try {
            start()
        } catch (error: Throwable) {
            markReaderExited()
            throw error
        }
        return true
    }

    fun markReaderExited() {
        clearFlag(READER_RUNNING)
    }

    /** Returns true to exactly one caller after teardown starts and the reader exits. */
    fun claimFinish(): Boolean {
        while (true) {
            val current = state.get()
            if (current and TEARDOWN_STARTED == 0 ||
                current and READER_RUNNING != 0 ||
                current and TEARDOWN_FINISHED != 0
            ) {
                return false
            }
            if (state.compareAndSet(current, current or TEARDOWN_FINISHED)) {
                return true
            }
        }
    }

    private fun setFlagUnlessPresent(flag: Int): Boolean {
        while (true) {
            val current = state.get()
            if (current and flag != 0) {
                return false
            }
            if (state.compareAndSet(current, current or flag)) {
                return true
            }
        }
    }

    private fun clearFlag(flag: Int) {
        while (true) {
            val current = state.get()
            if (current and flag == 0) {
                return
            }
            if (state.compareAndSet(current, current and flag.inv())) {
                return
            }
        }
    }

    private companion object {
        const val TEARDOWN_STARTED = 1 shl 0
        const val READER_RUNNING = 1 shl 1
        const val TEARDOWN_FINISHED = 1 shl 2
    }
}
