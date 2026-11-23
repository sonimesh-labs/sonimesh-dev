package com.sonimesh.opusspike

/** Process-local, main-thread status for the interactive lab. */
internal object LabStatus {
    val captureAdmission = LabCaptureAdmission()
    private var message = "Idle"
    private val observers = mutableSetOf<(String) -> Unit>()
    private var senderQueueDrops: Long? = null
    private val dropObservers = mutableSetOf<(Long?) -> Unit>()

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

    fun observeSenderQueueDrops(observer: (Long?) -> Unit) {
        dropObservers.add(observer)
        observer(senderQueueDrops)
    }

    fun removeSenderQueueDrops(observer: (Long?) -> Unit) {
        dropObservers.remove(observer)
    }

    fun publishSenderQueueDrops(value: Long?) {
        senderQueueDrops = value
        dropObservers.toList().forEach { it(value) }
    }
}
