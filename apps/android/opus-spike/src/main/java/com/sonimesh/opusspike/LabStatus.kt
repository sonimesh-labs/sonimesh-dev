package com.sonimesh.opusspike

/** Process-local, main-thread status for the interactive lab. */
internal object LabStatus {
    val captureAdmission = LabCaptureAdmission()
    var captureUsage: LabCaptureUsage? = null
        private set
    private var message = "Idle"
    private val observers = mutableSetOf<(String) -> Unit>()
    private var senderQueueDrops: Long? = null
    private val dropObservers = mutableSetOf<(Long?) -> Unit>()
    private var encoderRate: Long? = null
    private val rateObservers = mutableSetOf<(Long?) -> Unit>()

    fun serviceStarted(usage: LabCaptureUsage) {
        captureUsage = usage
        captureAdmission.serviceStarted()
    }

    fun serviceFinished() {
        captureUsage = null
        captureAdmission.serviceFinished()
    }

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

    fun observeEncoderRate(observer: (Long?) -> Unit) {
        rateObservers.add(observer)
        observer(encoderRate)
    }

    fun removeEncoderRate(observer: (Long?) -> Unit) {
        rateObservers.remove(observer)
    }

    fun publishEncoderRate(value: Long?) {
        encoderRate = value
        rateObservers.toList().forEach { it(value) }
    }
}
