package com.sonimesh.opusspike

import com.sonimesh.sender.encoder.OpusBitrate

internal data class LabSessionConfig(val usage: LabCaptureUsage, val bitrate: OpusBitrate)

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
    private var sentPackets: Long? = null
    private val sentPacketObservers = mutableSetOf<(Long?) -> Unit>()
    private var sessionConfig: LabSessionConfig? = null
    private val sessionObservers = mutableSetOf<(LabSessionConfig?) -> Unit>()

    fun serviceStarted(usage: LabCaptureUsage, bitrate: OpusBitrate) {
        captureUsage = usage
        captureAdmission.serviceStarted()
        sessionConfig = LabSessionConfig(usage, bitrate)
        sessionObservers.toList().forEach { it(sessionConfig) }
    }

    fun serviceFinished() {
        captureUsage = null
        captureAdmission.serviceFinished()
    }

    fun observeSessionConfig(observer: (LabSessionConfig?) -> Unit) {
        sessionObservers.add(observer)
        observer(sessionConfig)
    }

    fun removeSessionConfig(observer: (LabSessionConfig?) -> Unit) {
        sessionObservers.remove(observer)
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

    fun observeSentPackets(observer: (Long?) -> Unit) {
        sentPacketObservers.add(observer)
        observer(sentPackets)
    }

    fun removeSentPackets(observer: (Long?) -> Unit) {
        sentPacketObservers.remove(observer)
    }

    fun publishSentPackets(value: Long?) {
        sentPackets = value
        sentPacketObservers.toList().forEach { it(value) }
    }

    /** Returns a bounded operator summary without invitation or audio payloads. */
    fun copyableEvidence(): String = buildString {
        appendLine("SoniMesh paired audio lab")
        appendLine("Status: $message")
        sessionConfig?.let { config ->
            appendLine("Session request: ${config.usage.label}, ${config.bitrate.bitsPerSecond / 1_000} kbps target")
        }
        appendLine("Sender queue drops: ${senderQueueDrops ?: "—"}")
        appendLine("Media packets sent: ${sentPackets ?: "—"}")
        appendLine("Encoded payload average: ${encoderRate?.let { "$it kbps" } ?: "—"}")
    }.trimEnd()
}
