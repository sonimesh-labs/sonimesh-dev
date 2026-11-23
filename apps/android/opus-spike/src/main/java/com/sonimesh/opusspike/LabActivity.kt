package com.sonimesh.opusspike

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.text.method.DigitsKeyListener
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast
import com.sonimesh.sender.encoder.OpusBitrate
import com.sonimesh.sender.protocol.PairingQr
import com.journeyapps.barcodescanner.ScanOptions
import kotlin.math.PI
import kotlin.math.sin

@Suppress("DEPRECATION")
class LabActivity : Activity() {
    private var tone: AudioTrack? = null
    private var udpMode = false
    private var pairedMode = false
    private var pendingAddress = DEFAULT_LAB_ADDRESS
    private var pendingPin = ""
    private var pendingHost = "127.0.0.1"
    private var pendingUsage = LabCaptureUsage.All
    private var pendingBitrate = OpusBitrate.STANDARD
    private var importedInvitation: ByteArray? = null
    private var importedPin = ""
    private var importedHost = ""
    private lateinit var address: EditText
    private lateinit var invitation: EditText
    private lateinit var pin: EditText
    private lateinit var host: EditText
    private lateinit var status: TextView
    private lateinit var senderQueueDrops: TextView
    private lateinit var sentPackets: TextView
    private lateinit var encoderRate: TextView
    private lateinit var sessionConfig: TextView
    private lateinit var usage: Spinner
    private lateinit var bitrate: Spinner
    private val statusObserver: (String) -> Unit = { status.text = "Status: $it" }
    private val dropsObserver: (Long?) -> Unit = { count ->
        senderQueueDrops.text = if (count == null) "Sender queue drops: —" else "Sender queue drops: $count"
    }
    private val rateObserver: (Long?) -> Unit = { rate ->
        encoderRate.text = if (rate == null) "Encoded payload average: —" else "Encoded payload average: $rate kbps"
    }
    private val sentPacketsObserver: (Long?) -> Unit = { count ->
        sentPackets.text = if (count == null) "Media packets sent: —" else "Media packets sent: $count"
    }
    private val sessionObserver: (LabSessionConfig?) -> Unit = { config ->
        sessionConfig.text = if (config == null) "Session request: —" else
            "Session request: ${config.usage.label}, ${config.bitrate.bitsPerSecond / 1_000} kbps target"
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        tone = lastNonConfigurationInstance as? AudioTrack
        udpMode = savedInstanceState?.getBoolean("udpMode") ?: false
        pairedMode = savedInstanceState?.getBoolean("pairedMode") ?: false
        pendingAddress = savedInstanceState?.getString("pendingAddress") ?: DEFAULT_LAB_ADDRESS
        pendingPin = savedInstanceState?.getString("pendingPin") ?: ""
        pendingHost = savedInstanceState?.getString("pendingHost") ?: "127.0.0.1"
        pendingUsage = LabCaptureUsage.fromValue(savedInstanceState?.getString("pendingUsage"))
        val defaultBitrate = OpusBitrate.STANDARD.bitsPerSecond
        pendingBitrate = OpusBitrate.fromBitsPerSecond(savedInstanceState?.getInt("pendingBitrate", defaultBitrate) ?: defaultBitrate)
        savedInstanceState?.getString("importedInvitation")?.let { encoded ->
            runCatching { labHexToBytes(encoded) }.onSuccess { bytes ->
                importedInvitation = bytes
                importedPin = savedInstanceState.getString("importedPin") ?: ""
                importedHost = savedInstanceState.getString("importedHost") ?: ""
            }
        }
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setOnApplyWindowInsetsListener { view, insets ->
                val bars = insets.getInsets(WindowInsets.Type.systemBars())
                view.setPadding(24 + bars.left, 24 + bars.top, 24 + bars.right, 24 + bars.bottom)
                insets
            }
        }
        layout.addView(TextView(this).apply {
            text = "Audio lab: start the desktop listener first. Capture can be silent if the source disallows it."
        })
        status = TextView(this).apply { accessibilityLiveRegion = android.view.View.ACCESSIBILITY_LIVE_REGION_POLITE }
        layout.addView(status)
        senderQueueDrops = TextView(this)
        layout.addView(senderQueueDrops)
        sentPackets = TextView(this)
        layout.addView(sentPackets)
        encoderRate = TextView(this)
        layout.addView(encoderRate)
        sessionConfig = TextView(this)
        layout.addView(sessionConfig)
        layout.addView(Button(this).apply {
            text = "Copy lab evidence"
            setOnClickListener { copyLabEvidence() }
        })
        layout.addView(Button(this).apply {
            text = "Share lab evidence"
            setOnClickListener { shareLabEvidence() }
        })
        layout.addView(TextView(this).apply { text = "Playback usage for the next capture" })
        usage = Spinner(this).apply {
            contentDescription = "Playback usage for capture"
            adapter = ArrayAdapter(
                this@LabActivity,
                android.R.layout.simple_spinner_dropdown_item,
                LabCaptureUsage.entries.map { it.label },
            )
            val selected = LabCaptureUsage.fromValue(savedInstanceState?.getString("selectedUsage"))
            setSelection(LabCaptureUsage.entries.indexOf(selected))
        }
        layout.addView(usage)
        layout.addView(TextView(this).apply { text = "Opus target bitrate for the next capture" })
        bitrate = Spinner(this).apply {
            contentDescription = "Opus target bitrate for capture"
            adapter = ArrayAdapter(
                this@LabActivity,
                android.R.layout.simple_spinner_dropdown_item,
                OpusBitrate.entries.map { "${it.bitsPerSecond / 1_000} kbps" },
            )
            val selected = OpusBitrate.fromBitsPerSecond(savedInstanceState?.getInt("selectedBitrate", defaultBitrate) ?: defaultBitrate)
            setSelection(OpusBitrate.entries.indexOf(selected))
        }
        layout.addView(bitrate)
        layout.addView(Button(this).apply {
            text = "Start ADB capture"
            setOnClickListener { startCapture(false) }
        })
        layout.addView(TextView(this).apply { text = "Desktop IPv4 for UDP (emulator: 10.0.2.2)" })
        address = EditText(this).apply {
            hint = "Desktop IPv4 address (10.0.2.2 for emulator)"
            contentDescription = "Desktop IPv4 address for UDP"
            keyListener = DigitsKeyListener.getInstance("0123456789.")
            setSingleLine(true)
            setText(savedInstanceState?.getString("address")
                ?: getPreferences(MODE_PRIVATE).getString("desktopAddress", DEFAULT_LAB_ADDRESS))
        }
        layout.addView(address)
        layout.addView(TextView(this).apply {
            text = "Lab invitation payload hex (optional; from the desktop listener)"
        })
        invitation = EditText(this).apply {
            hint = "Paste INVITATION_HEX"
            contentDescription = "Lab invitation payload"
            setSingleLine(true)
            setText(savedInstanceState?.getString("invitation") ?: "")
        }
        layout.addView(invitation)
        layout.addView(Button(this).apply {
            text = "Scan lab invitation"
            setOnClickListener { scanInvitation(startAfterScan = false) }
        })
        layout.addView(Button(this).apply {
            text = "Scan and start paired capture"
            setOnClickListener { scanInvitation(startAfterScan = true) }
        })
        layout.addView(Button(this).apply {
            text = "Paste lab invitation"
            setOnClickListener { pasteInvitation() }
        })
        layout.addView(Button(this).apply {
            text = "Apply invitation payload"
            setOnClickListener { applyInvitation() }
        })
        layout.addView(Button(this).apply {
            text = "Apply invitation and use address for UDP"
            setOnClickListener { applyInvitation(useAddressForUdp = true) }
        })
        layout.addView(Button(this).apply {
            text = "Apply invitation and start paired capture"
            setOnClickListener {
                if (!LabStatus.captureAdmission.canBegin()) {
                    showCaptureBusy()
                } else if (applyInvitation(useAddressForUdp = true)) {
                    startCapture(true, paired = true)
                }
            }
        })
        layout.addView(Button(this).apply {
            text = "Start UDP capture"
            setOnClickListener { startCapture(true) }
        })
        layout.addView(TextView(this).apply { text = "Paired lab desktop SPKI_HEX (from the listener)" })
        pin = EditText(this).apply {
            hint = "Paste desktop SPKI_HEX"
            contentDescription = "Paired lab desktop SPKI"
            setSingleLine(true)
            setText(savedInstanceState?.getString("pin") ?: "")
        }
        layout.addView(pin)
        layout.addView(Button(this).apply {
            text = "Paste desktop SPKI"
            setOnClickListener { pasteDesktopSpki() }
        })
        layout.addView(TextView(this).apply { text = "Paired control IPv4 (ADB reverse: 127.0.0.1)" })
        host = EditText(this).apply {
            hint = "Desktop control IPv4 address"
            contentDescription = "Paired control IPv4 address"
            keyListener = DigitsKeyListener.getInstance("0123456789.")
            setSingleLine(true)
            setText(savedInstanceState?.getString("host") ?: "127.0.0.1")
        }
        layout.addView(host)
        layout.addView(Button(this).apply {
            text = "Use UDP address for control"
            setOnClickListener {
                val parsed = parseLabAddress(address.text.toString())
                if (parsed == null) {
                    LabStatus.publish("Enter a valid desktop IPv4 address for UDP.")
                } else {
                    host.setText(parsed.hostAddress)
                }
            }
        })
        layout.addView(Button(this).apply {
            text = "Use control address for UDP"
            setOnClickListener {
                val parsed = parseLabAddress(host.text.toString())
                if (parsed == null) {
                    LabStatus.publish("Enter a non-loopback control IPv4 address before copying it to UDP.")
                } else {
                    address.setText(parsed.hostAddress)
                }
            }
        })
        layout.addView(Button(this).apply {
            text = "Start paired UDP capture"
            setOnClickListener { startCapture(true, paired = true) }
        })
        layout.addView(Button(this).apply {
            text = "Stop capture"
            setOnClickListener {
                startService(Intent(this@LabActivity, LabCaptureService::class.java).setAction("stop"))
            }
        })
        layout.addView(TextView(this).apply {
            text = "New test tones use the current capture usage, or the selection when capture is stopped. Restart a playing tone to apply changes."
        })
        layout.addView(Button(this).apply {
            text = if (tone == null) "Start test tone" else "Stop test tone"
            setOnClickListener {
                toggleTone()
                text = if (tone == null) "Start test tone" else "Stop test tone"
            }
        })
        setContentView(ScrollView(this).apply { addView(layout) })
        handleIncomingInvitation(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncomingInvitation(intent)
    }

    override fun onStart() {
        super.onStart()
        LabStatus.observe(statusObserver)
        LabStatus.observeSenderQueueDrops(dropsObserver)
        LabStatus.observeSentPackets(sentPacketsObserver)
        LabStatus.observeEncoderRate(rateObserver)
        LabStatus.observeSessionConfig(sessionObserver)
    }

    override fun onStop() {
        LabStatus.remove(statusObserver)
        LabStatus.removeSenderQueueDrops(dropsObserver)
        LabStatus.removeSentPackets(sentPacketsObserver)
        LabStatus.removeEncoderRate(rateObserver)
        LabStatus.removeSessionConfig(sessionObserver)
        super.onStop()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putBoolean("udpMode", udpMode)
        outState.putBoolean("pairedMode", pairedMode)
        outState.putString("pendingAddress", pendingAddress)
        outState.putString("pendingPin", pendingPin)
        outState.putString("pendingHost", pendingHost)
        outState.putString("pendingUsage", pendingUsage.value)
        outState.putString("selectedUsage", selectedUsage().value)
        outState.putInt("pendingBitrate", pendingBitrate.bitsPerSecond)
        outState.putInt("selectedBitrate", selectedBitrate().bitsPerSecond)
        importedInvitation?.let { outState.putString("importedInvitation", labHex(it)) }
        outState.putString("importedPin", importedPin)
        outState.putString("importedHost", importedHost)
        outState.putString("address", address.text.toString())
        outState.putString("invitation", invitation.text.toString())
        outState.putString("pin", pin.text.toString())
        outState.putString("host", host.text.toString())
        super.onSaveInstanceState(outState)
    }

    private fun startCapture(udp: Boolean, paired: Boolean = false) {
        if (!LabStatus.captureAdmission.canBegin()) {
            showCaptureBusy()
            return
        }
        if (udp) {
            val parsed = parseLabAddress(address.text.toString())
            if (parsed == null) {
                LabStatus.publish("Enter a valid desktop IPv4 address for UDP.")
                return
            }
            pendingAddress = requireNotNull(parsed.hostAddress)
            getPreferences(MODE_PRIVATE).edit().putString("desktopAddress", pendingAddress).apply()
        }
        if (paired) {
            val enteredHost = host.text.toString().trim()
            val parsedHost = if (enteredHost == "127.0.0.1") enteredHost else parseLabAddress(enteredHost)?.hostAddress
            if (parsedHost == null) {
                LabStatus.publish("Enter a valid paired control IPv4 address.")
                return
            }
            val entered = pin.text.toString().trim()
            if (runCatching { labInvitation(entered, parsedHost) }.isFailure) {
                LabStatus.publish("Paste the desktop listener's complete SPKI_HEX.")
                return
            }
            if (importedInvitation != null && (entered != importedPin || parsedHost != importedHost)) {
                importedInvitation?.fill(0)
                importedInvitation = null
            }
            pendingPin = entered
            pendingHost = parsedHost
        }
        udpMode = udp
        pairedMode = paired
        pendingUsage = selectedUsage()
        pendingBitrate = selectedBitrate()
        val needsPermission = checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED
        val needsNotification = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        if (!LabStatus.captureAdmission.begin(needsPermission, needsNotification)) {
            showCaptureBusy()
            return
        }
        if (needsPermission) {
            try {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 1)
            } catch (_: RuntimeException) {
                LabStatus.captureAdmission.launchFailed()
                LabStatus.publish("Unable to request microphone permission. Try again.")
            }
        } else if (needsNotification) requestNotificationPermission() else requestProjection()
    }

    private fun selectedUsage(): LabCaptureUsage =
        LabCaptureUsage.entries.getOrElse(usage.selectedItemPosition) { LabCaptureUsage.All }

    private fun selectedBitrate(): OpusBitrate =
        OpusBitrate.entries.getOrElse(bitrate.selectedItemPosition) { OpusBitrate.STANDARD }

    private fun showCaptureBusy() {
        Toast.makeText(this, "Capture is already starting, running, or stopping.", Toast.LENGTH_SHORT).show()
    }

    private fun copyLabEvidence() {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(
            ClipData.newPlainText("SoniMesh lab evidence", LabStatus.copyableEvidence()),
        )
        Toast.makeText(this, "Lab evidence copied.", Toast.LENGTH_SHORT).show()
    }

    private fun shareLabEvidence() {
        try {
            startActivity(
                Intent.createChooser(
                    buildLabEvidenceShareIntent(LabStatus.copyableEvidence()),
                    "Share lab evidence",
                ),
            )
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(this, "No app is available to share lab evidence.", Toast.LENGTH_SHORT).show()
        }
    }

    private fun scanInvitation(startAfterScan: Boolean) {
        if (startAfterScan && !LabStatus.captureAdmission.canBegin()) {
            showCaptureBusy()
            return
        }
        val options = ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt("Scan the lab invitation on the desktop")
            .setBeepEnabled(false)
        startActivityForResult(options.createScanIntent(this), if (startAfterScan) 4 else 3)
    }

    private fun pasteInvitation() {
        val text = getSystemService(ClipboardManager::class.java).primaryClip
            ?.takeIf { it.itemCount > 0 }
            ?.getItemAt(0)
            ?.coerceToText(this)
            ?.toString()
        setInvitationText(text, "Invitation pasted. Apply it to continue.", "Copy the lab invitation first.")
    }

    private fun handleIncomingInvitation(source: Intent?) {
        if (source?.action != Intent.ACTION_SEND || source.type != "text/plain") return
        setInvitationText(
            source.getStringExtra(Intent.EXTRA_TEXT),
            "Invitation received from share. Apply it to continue.",
            "The shared invitation is empty.",
        )
    }

    private fun setInvitationText(value: String?, successMessage: String, emptyMessage: String) {
        val text = value?.trim()
        when {
            text.isNullOrEmpty() -> LabStatus.publish(emptyMessage)
            text.length > PairingQr.MAX_PAYLOAD_BYTES * 2 -> LabStatus.publish("The lab invitation is too long.")
            else -> {
                invitation.setText(text)
                LabStatus.publish(successMessage)
            }
        }
    }

    private fun pasteDesktopSpki() {
        val text = getSystemService(ClipboardManager::class.java).primaryClip
            ?.takeIf { it.itemCount > 0 }
            ?.getItemAt(0)
            ?.coerceToText(this)
            ?.toString()
            ?.trim()
        if (text.isNullOrEmpty()) {
            LabStatus.publish("Copy the desktop SPKI_HEX first.")
            return
        }
        try {
            labInvitation(text).fill(0)
            pin.setText(text)
            LabStatus.publish("Desktop SPKI pasted. Enter the control address and start paired capture.")
        } catch (_: IllegalArgumentException) {
            LabStatus.publish("The copied desktop SPKI_HEX is invalid.")
        }
    }

    private fun applyInvitation(useAddressForUdp: Boolean = false): Boolean {
        var raw: ByteArray? = null
        var retained = false
        try {
            raw = labHexToBytes(invitation.text.toString())
            val decoded = PairingQr.decode(requireNotNull(raw))
            try {
                require(decoded.controlPort == 47610)
                val udpAddress = parseLabAddress(decoded.controlHost)
                require(decoded.controlHost == "127.0.0.1" || udpAddress != null)
                val decodedPin = labHex(decoded.desktopTlsSpki)
                pin.setText(decodedPin)
                host.setText(decoded.controlHost)
                if (useAddressForUdp && udpAddress != null) address.setText(udpAddress.hostAddress)
                importedInvitation?.fill(0)
                importedInvitation = raw
                importedPin = decodedPin
                importedHost = decoded.controlHost
                retained = true
                invitation.setText("")
                LabStatus.publish(
                    if (useAddressForUdp && udpAddress != null) "Invitation applied. UDP uses the same address."
                    else "Invitation applied. Check the desktop IPv4 for UDP separately.",
                )
            } finally {
                decoded.pairingId.fill(0)
                decoded.pairingSecret.fill(0)
                decoded.desktopTlsSpki.fill(0)
            }
        } catch (_: IllegalArgumentException) {
            LabStatus.publish("Paste a valid lab invitation payload.")
        } finally {
            if (!retained) raw?.fill(0)
        }
        return retained
    }

    internal fun applyScannedInvitation(contents: String?, startAfterScan: Boolean = false) {
        if (contents == null || contents.length > PairingQr.MAX_PAYLOAD_BYTES * 2) {
            LabStatus.publish("Scan a valid lab invitation, or paste its payload.")
            return
        }
        invitation.setText(contents)
        if (applyInvitation(useAddressForUdp = true) && startAfterScan) {
            startCapture(true, paired = true)
        }
    }

    private fun onScannerResult(resultCode: Int, contents: String?, startAfterScan: Boolean) {
        if (resultCode == RESULT_OK) applyScannedInvitation(contents, startAfterScan)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 1) {
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            if (!LabStatus.captureAdmission.permissionResult(granted)) return
            if (granted) {
                if (LabStatus.captureAdmission.awaitingNotification()) requestNotificationPermission() else requestProjection()
            }
            else LabStatus.publish("Microphone permission is required for playback capture.")
        } else if (requestCode == 5) {
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            if (!LabStatus.captureAdmission.notificationResult(granted)) return
            if (granted) requestProjection()
            else LabStatus.publish("Notification permission is required to show capture controls. Enable it in app settings and try again.")
        }
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        try {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 5)
        } catch (_: RuntimeException) {
            LabStatus.captureAdmission.launchFailed()
            LabStatus.publish("Unable to request notification permission. Try again.")
        }
    }

    private fun requestProjection() {
        try {
            val manager = getSystemService(MediaProjectionManager::class.java)
            startActivityForResult(manager.createScreenCaptureIntent(), 2)
        } catch (_: RuntimeException) {
            LabStatus.captureAdmission.launchFailed()
            LabStatus.publish("Unable to request screen capture. Try again.")
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == 3 || requestCode == 4) {
            onScannerResult(resultCode, data?.getStringExtra("SCAN_RESULT"), startAfterScan = requestCode == 4)
            return
        }
        if (requestCode == 2) {
            val granted = resultCode == RESULT_OK && data != null
            if (!LabStatus.captureAdmission.projectionResult(granted)) return
            if (!granted) {
                LabStatus.publish("Screen capture consent was not granted.")
                return
            }
            try {
                startForegroundService(Intent(this, LabCaptureService::class.java).apply {
                    putExtra("resultCode", resultCode)
                    putExtra("resultData", data)
                    putExtra("udpMode", udpMode)
                    putExtra("udpAddress", pendingAddress)
                    putExtra("pairedMode", pairedMode)
                    putExtra("captureUsage", pendingUsage.value)
                    putExtra("bitrate", pendingBitrate.bitsPerSecond)
                    if (pairedMode) {
                        putExtra("pairedPin", pendingPin)
                        putExtra("pairedHost", pendingHost)
                        importedInvitation?.let { putExtra("pairedInvitation", it.copyOf()) }
                    }
                })
            } catch (_: RuntimeException) {
                LabStatus.captureAdmission.launchFailed()
                LabStatus.publish("Unable to start capture. Try again.")
            }
            importedInvitation?.fill(0)
            importedInvitation = null
        }
    }

    private fun toggleTone() {
        tone?.let { it.stop(); it.release(); tone = null; return }
        val samples = ShortArray(48_000 * 2) { index ->
            (sin(2.0 * PI * (index / 2 % 48) / 48.0) * 3_000).toInt().toShort()
        }
        val track = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage((LabStatus.captureUsage ?: selectedUsage()).usages.first()).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(48_000)
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_STEREO).build())
            .setTransferMode(AudioTrack.MODE_STATIC)
            .setBufferSizeInBytes(samples.size * Short.SIZE_BYTES)
            .build()
        track.write(samples, 0, samples.size)
        track.setLoopPoints(0, 48_000, -1)
        track.play()
        tone = track
    }

    override fun onRetainNonConfigurationInstance(): Any? = tone

    internal fun isTestTonePlaying(): Boolean = tone?.playState == AudioTrack.PLAYSTATE_PLAYING

    internal fun testToneUsage(): Int? = tone?.audioAttributes?.usage

    override fun onDestroy() {
        importedInvitation?.fill(0)
        importedInvitation = null
        if (!isChangingConfigurations) tone?.let { it.stop(); it.release() }
        tone = null
        super.onDestroy()
    }
}

internal fun buildLabEvidenceShareIntent(evidence: String): Intent =
    Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_TEXT, evidence)
    }
