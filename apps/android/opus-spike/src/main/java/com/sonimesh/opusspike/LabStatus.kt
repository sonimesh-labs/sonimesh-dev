package com.sonimesh.opusspike

import com.sonimesh.sender.encoder.OpusBitrate

internal enum class LabTransport(val label: String) {
    Adb("ADB TCP"),
    Udp("UDP"),
    PairedUdp("paired UDP"),

    ;

    companion object {
        fun fromFlags(udpMode: Boolean, pairedMode: Boolean): LabTransport = when {
            pairedMode -> PairedUdp
            udpMode -> Udp
            else -> Adb
        }
    }
}

internal data class LabSessionConfig(
    val usage: LabCaptureUsage,
    val bitrate: OpusBitrate,
    val transport: LabTransport,
    val udpPort: Int? = null,
)

internal enum class LabSignalState(val label: String) {
    Silent("no capturable audio"),
    Detected("signal detected"),
}

/** Process-local, main-thread status for the interactive lab. */
internal object LabStatus {
    private const val MAX_MESSAGE_CHARS = 256
    val captureAdmission = LabCaptureAdmission()
    var captureUsage: LabCaptureUsage? = null
        private set
    var retryAvailable = false
        private set
    private var message = "Idle"
    private val observers = mutableSetOf<(String) -> Unit>()
    private var senderQueueDrops: Long? = null
    private val dropObservers = mutableSetOf<(Long?) -> Unit>()
    private var encoderRate: Long? = null
    private val rateObservers = mutableSetOf<(Long?) -> Unit>()
    private var sentPackets: Long? = null
    private var senderTiming: SenderTimingSnapshot? = null
    private val sentPacketObservers = mutableSetOf<(Long?) -> Unit>()
    private var senderInterval: SenderInterval? = null
    private val senderIntervalObservers = mutableSetOf<(SenderInterval?) -> Unit>()
    private var sessionConfig: LabSessionConfig? = null
    val retryTransport: LabTransport?
        get() = if (retryAvailable) sessionConfig?.transport else null
    private val sessionObservers = mutableSetOf<(LabSessionConfig?) -> Unit>()
    private var signalState: LabSignalState? = null
    private val signalObservers = mutableSetOf<(LabSignalState?) -> Unit>()
    private var captureDurationSeconds: Long? = null
    private val durationObservers = mutableSetOf<(Long?) -> Unit>()
    private var senderMuted: Boolean? = null
    private var lastSenderMuted: Boolean? = null
    private val muteObservers = mutableSetOf<(Boolean?) -> Unit>()
    private var senderGain: Int? = null
    private var lastSenderGain: Int? = null
    private val gainObservers = mutableSetOf<(Int?) -> Unit>()
    private var clippedFrames: Long? = null
    private val clippingObservers = mutableSetOf<(Long?) -> Unit>()
    private var clippedInterval: ClippingInterval? = null
    private val clippingIntervalObservers = mutableSetOf<(ClippingInterval?) -> Unit>()

    fun serviceStarted(usage: LabCaptureUsage, bitrate: OpusBitrate, transport: LabTransport = LabTransport.Adb, udpPort: Int? = null) {
        captureUsage = usage
        captureAdmission.serviceStarted()
        retryAvailable = false
        observers.toList().forEach { it(message) }
        sessionConfig = LabSessionConfig(usage, bitrate, transport, udpPort)
        signalState = null
        captureDurationSeconds = null
        senderTiming = null
        publishSenderInterval(null)
        publishSenderMuted(false)
        publishSenderGain(DEFAULT_SENDER_GAIN)
        publishClippedFrames(null)
        publishClippedInterval(null)
        sessionObservers.toList().forEach { it(sessionConfig) }
        signalObservers.toList().forEach { it(null) }
        durationObservers.toList().forEach { it(null) }
    }

    fun serviceFinished() {
        val hadCapture = captureUsage != null
        captureUsage = null
        captureAdmission.serviceFinished()
        publishSenderMuted(null)
        publishSenderGain(null)
        if (hadCapture) {
            retryAvailable = true
            observers.toList().forEach { it(message) }
        }
    }

    fun observeSenderMuted(observer: (Boolean?) -> Unit) {
        muteObservers.add(observer)
        observer(senderMuted)
    }

    fun observeSenderGain(observer: (Int?) -> Unit) {
        gainObservers.add(observer)
        observer(senderGain)
    }

    fun observeClippedFrames(observer: (Long?) -> Unit) {
        clippingObservers.add(observer)
        observer(clippedFrames)
    }

    fun removeClippedFrames(observer: (Long?) -> Unit) {
        clippingObservers.remove(observer)
    }

    fun publishClippedFrames(value: Long?) {
        clippedFrames = value
        clippingObservers.toList().forEach { it(value) }
    }

    fun observeClippedInterval(observer: (ClippingInterval?) -> Unit) {
        clippingIntervalObservers.add(observer)
        observer(clippedInterval)
    }

    fun removeClippedInterval(observer: (ClippingInterval?) -> Unit) {
        clippingIntervalObservers.remove(observer)
    }

    fun publishClippedInterval(value: ClippingInterval?) {
        clippedInterval = value
        clippingIntervalObservers.toList().forEach { it(value) }
    }

    fun removeSenderGain(observer: (Int?) -> Unit) {
        gainObservers.remove(observer)
    }

    fun publishSenderGain(value: Int?) {
        senderGain = value
        if (value != null) lastSenderGain = value
        gainObservers.toList().forEach { it(value) }
    }

    fun removeSenderMuted(observer: (Boolean?) -> Unit) {
        muteObservers.remove(observer)
    }

    fun publishSenderMuted(value: Boolean?) {
        senderMuted = value
        if (value != null) lastSenderMuted = value
        muteObservers.toList().forEach { it(value) }
    }

    fun observeSessionConfig(observer: (LabSessionConfig?) -> Unit) {
        sessionObservers.add(observer)
        observer(sessionConfig)
    }

    fun removeSessionConfig(observer: (LabSessionConfig?) -> Unit) {
        sessionObservers.remove(observer)
    }

    fun observeSignal(observer: (LabSignalState?) -> Unit) {
        signalObservers.add(observer)
        observer(signalState)
    }

    fun removeSignal(observer: (LabSignalState?) -> Unit) {
        signalObservers.remove(observer)
    }

    fun publishSignal(value: LabSignalState?) {
        signalState = value
        signalObservers.toList().forEach { it(value) }
    }

    fun observeCaptureDuration(observer: (Long?) -> Unit) {
        durationObservers.add(observer)
        observer(captureDurationSeconds)
    }

    fun removeCaptureDuration(observer: (Long?) -> Unit) {
        durationObservers.remove(observer)
    }

    fun publishCaptureDuration(value: Long?) {
        captureDurationSeconds = value?.coerceAtLeast(0)
        durationObservers.toList().forEach { it(captureDurationSeconds) }
    }

    fun observe(observer: (String) -> Unit) {
        observers.add(observer)
        observer(message)
    }

    fun remove(observer: (String) -> Unit) {
        observers.remove(observer)
    }

    fun publish(value: String) {
        message = boundMessage(value)
        observers.toList().forEach { it(message) }
    }

    fun publishIfIdle(value: String) {
        if (message == "Idle" && captureAdmission.canBegin()) publish(value)
    }

    private fun boundMessage(value: String): String {
        if (value.length <= MAX_MESSAGE_CHARS) return value
        return value.take(MAX_MESSAGE_CHARS - 3) + "..."
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

    fun observeSenderInterval(observer: (SenderInterval?) -> Unit) {
        senderIntervalObservers.add(observer)
        observer(senderInterval)
    }

    fun removeSenderInterval(observer: (SenderInterval?) -> Unit) {
        senderIntervalObservers.remove(observer)
    }

    fun publishSenderInterval(value: SenderInterval?) {
        senderInterval = value
        senderIntervalObservers.toList().forEach { it(value) }
    }

    fun publishSenderTiming(value: SenderTimingSnapshot?) {
        senderTiming = value
    }

    /** Returns a bounded operator summary without invitation or audio payloads. */
    fun copyableEvidence(): String = buildString {
        appendLine("SoniMesh paired audio lab")
        appendLine("Status: $message")
        sessionConfig?.let { config ->
            appendLine("Session request: ${config.usage.label}, ${config.bitrate.bitsPerSecond / 1_000} kbps target")
            appendLine("Transport: ${config.transport.label}")
            config.udpPort?.let { appendLine("Requested UDP port: $it") }
        }
        appendLine("Capture signal: ${signalState?.label ?: "—"}")
        appendLine("Capture duration: ${captureDurationSeconds?.let { "${it}s" } ?: "—"}")
        appendLine("Sending mute (last known): ${lastSenderMuted?.let { if (it) "on" else "off" } ?: "—"}")
        appendLine("Sender gain (last known): ${lastSenderGain?.let { "$it%" } ?: "—"}")
        appendLine("Frames clipped by sender gain: ${clippedFrames ?: "—"}")
        appendLine("Clipped frames in last completed interval: ${clippedInterval?.let { "${it.clipped} of ${it.accepted}" } ?: "—"}")
        appendLine("Sender queue drops: ${senderQueueDrops ?: "—"}")
        appendLine("Media packets sent: ${sentPackets ?: "—"}")
        appendLine("Last local send interval: ${senderInterval?.let { "${it.packets} packets in ${it.millis} ms" } ?: "—"}")
        appendLine("Send intervals under 10 ms: ${senderTiming?.let { "${it.underTenMillis} of ${it.intervals}" } ?: "—"}")
        appendLine("Encoded payload average: ${encoderRate?.let { "$it kbps" } ?: "—"}")
    }.trimEnd()
}
