package com.sonimesh.opusspike

/** Emits changes in capture signal state without retaining audio samples. */
internal class LabSignalMonitor {
    private var silentFrames = 0
    private var lastTransition: LabSignalTransition? = null

    fun observe(samples: ShortArray, offset: Int, count: Int): LabSignalTransition? {
        require(offset >= 0 && offset % CHANNELS == 0 && count > 0 && count % CHANNELS == 0 && offset <= samples.size - count)
        val silent = (offset until offset + count).all { samples[it] == 0.toShort() }
        if (silent) {
            silentFrames += minOf(SILENT_FRAMES - silentFrames, count / CHANNELS)
            if (lastTransition != LabSignalTransition.NoCapturableAudio && silentFrames == SILENT_FRAMES) {
                lastTransition = LabSignalTransition.NoCapturableAudio
                return LabSignalTransition.NoCapturableAudio
            }
        } else {
            silentFrames = 0
            if (lastTransition != LabSignalTransition.SignalDetected) {
                lastTransition = LabSignalTransition.SignalDetected
                return LabSignalTransition.SignalDetected
            }
        }
        return null
    }

    private companion object {
        const val CHANNELS = 2
        const val SILENT_FRAMES = 48_000 * 2
    }
}

internal enum class LabSignalTransition { NoCapturableAudio, SignalDetected }
