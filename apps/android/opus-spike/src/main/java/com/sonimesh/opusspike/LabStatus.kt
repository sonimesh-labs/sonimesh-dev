package com.sonimesh.opusspike

/** Process-local, main-thread status for the interactive lab. */
internal object LabStatus {
    val captureAdmission = LabCaptureAdmission()
    private var message = "Idle"
    private val observers = mutableSetOf<(String) -> Unit>()

    fun observe(observer: (String) -> Unit) {
        observers.add(observer)
        observer(message)
    }

    fun remove(observer: (String) -> Unit) {
        observers.remove(observer)
    }

    fun publish(value: String) {
        message = value
        observers.toList().forEach { it(value) }
    }
}
