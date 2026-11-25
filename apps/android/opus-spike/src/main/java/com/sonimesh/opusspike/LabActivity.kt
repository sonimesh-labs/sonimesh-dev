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
import android.os.Handler
import android.os.Looper
import android.text.method.DigitsKeyListener
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.AdapterView
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import android.view.View
import com.sonimesh.sender.encoder.OpusBitrate
import com.sonimesh.sender.protocol.PairingQr
import com.journeyapps.barcodescanner.ScanOptions

@Suppress("DEPRECATION")
class LabActivity : Activity() {
    private data class TestToneFormat(val sampleRate: Int, val channelMask: Int, val channels: Int)

    private val testToneFormats = listOf(
        TestToneFormat(48_000, AudioFormat.CHANNEL_OUT_STEREO, 2),
        TestToneFormat(44_100, AudioFormat.CHANNEL_OUT_STEREO, 2),
        TestToneFormat(48_000, AudioFormat.CHANNEL_OUT_MONO, 1),
        TestToneFormat(44_100, AudioFormat.CHANNEL_OUT_MONO, 1),
    )
    private var tone: AudioTrack? = null
    private var latencyProbe: AudioTrack? = null
    private val probeHandler = Handler(Looper.getMainLooper())
    private var probeTimeout: Runnable? = null
    private var udpMode = false
    private var pairedMode = false
    private var pendingAddress = DEFAULT_LAB_ADDRESS
    private var pendingMediaPort = DEFAULT_LAB_MEDIA_PORT
    private var pendingPin = ""
    private var pendingHost = "127.0.0.1"
    private var pendingControlPort = DEFAULT_LAB_CONTROL_PORT
    private var pendingUsage = LabCaptureUsage.All
    private var pendingBitrate = OpusBitrate.STANDARD
    private var importedInvitation: ByteArray? = null
    private var pendingInvitation: ByteArray? = null
    private var importedPin = ""
    private var importedHost = ""
    private var importedControlPort = DEFAULT_LAB_CONTROL_PORT
    private lateinit var address: EditText
    private lateinit var mediaPort: EditText
    private lateinit var invitation: EditText
    private lateinit var pin: EditText
    private lateinit var host: EditText
    private lateinit var controlPort: EditText
    private lateinit var status: TextView
    private lateinit var setupFeedback: TextView
    private lateinit var signal: TextView
    private lateinit var duration: TextView
    private lateinit var senderQueueDrops: TextView
    private lateinit var sentPackets: TextView
    private lateinit var encoderRate: TextView
    private lateinit var sessionConfig: TextView
    private lateinit var usage: Spinner
    private lateinit var bitrate: Spinner
    private lateinit var muteSwitch: Switch
    private lateinit var toneButton: Button
    private lateinit var probeButton: Button
    private lateinit var retryButton: Button
    private lateinit var retryHint: TextView
    private var updatingMute = false
    private var observedCapture = false
    private val muteObserver: (Boolean?) -> Unit = { muted ->
        updatingMute = true
        muteSwitch.isEnabled = muted != null
        muteSwitch.isChecked = muted == true
        updatingMute = false
    }
    private val statusObserver: (String) -> Unit = {
        val capturing = LabStatus.captureUsage != null
        if (observedCapture && !capturing) {
            stopTone()
            stopLatencyProbe()
            toneButton.text = "Start test tone"
        }
        observedCapture = capturing
        status.text = "Status: $it"
        updateRetryButton()
    }
    private val signalObserver: (LabSignalState?) -> Unit = { state ->
        signal.text = "Capture signal: ${state?.label ?: "—"}"
    }
    private val durationObserver: (Long?) -> Unit = { seconds ->
        duration.text = "Capture duration: ${seconds?.let { "${it}s" } ?: "—"}"
    }
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
            "Session request: ${config.usage.label}, ${config.bitrate.bitsPerSecond / 1_000} kbps target\nTransport: ${config.transport.label}" +
                (config.udpPort?.let { "\nRequested UDP port: $it" } ?: "")
    }
    private val labPreferences by lazy { getPreferences(MODE_PRIVATE) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val savedInvitation = savedInstanceState?.getString("importedInvitation")
            ?: labPreferences.getString(PREF_INVITATION, null)
        tone = lastNonConfigurationInstance as? AudioTrack
        udpMode = savedInstanceState?.getBoolean("udpMode") ?: false
        pairedMode = savedInstanceState?.getBoolean("pairedMode") ?: false
        pendingAddress = savedInstanceState?.getString("pendingAddress") ?: DEFAULT_LAB_ADDRESS
        pendingMediaPort = savedInstanceState?.getInt("pendingMediaPort", DEFAULT_LAB_MEDIA_PORT) ?: DEFAULT_LAB_MEDIA_PORT
        pendingPin = savedInstanceState?.getString("pendingPin") ?: ""
        pendingHost = savedInstanceState?.getString("pendingHost") ?: "127.0.0.1"
        pendingControlPort = savedInstanceState?.getInt("pendingControlPort", DEFAULT_LAB_CONTROL_PORT) ?: DEFAULT_LAB_CONTROL_PORT
        pendingInvitation = if (LabStatus.captureAdmission.canBegin()) null
            else savedInstanceState?.getByteArray("pendingInvitation")?.copyOf()
        pendingUsage = LabCaptureUsage.fromValue(savedInstanceState?.getString("pendingUsage"))
        val defaultBitrate = OpusBitrate.STANDARD.bitsPerSecond
        pendingBitrate = OpusBitrate.fromBitsPerSecond(savedInstanceState?.getInt("pendingBitrate", defaultBitrate) ?: defaultBitrate)
        savedInvitation?.let { encoded ->
            runCatching { labHexToBytes(encoded) }.onSuccess { bytes ->
                try {
                    val decoded = PairingQr.decode(bytes)
                    try {
                        importedInvitation = bytes
                        importedPin = labHex(decoded.desktopTlsSpki)
                        importedHost = decoded.controlHost
                        importedControlPort = decoded.controlPort
                    } finally {
                        decoded.pairingId.fill(0)
                        decoded.pairingSecret.fill(0)
                        decoded.desktopTlsSpki.fill(0)
                    }
                } catch (_: IllegalArgumentException) {
                    bytes.fill(0)
                }
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
        setupFeedback = TextView(this).apply {
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
            text = savedInstanceState?.getString("setupFeedback") ?: ""
        }
        layout.addView(setupFeedback)
        layout.addView(Button(this).apply {
            text = "Start paired UDP capture"
            setOnClickListener { startCapture(true, paired = true) }
        })
        layout.addView(Button(this).apply {
            text = "Stop capture"
            setOnClickListener {
                if (LabStatus.captureAdmission.cancelPending()) {
                    clearPendingInvitation()
                    LabStatus.publish("Capture start cancelled. Close the permission or screen-capture dialog before retrying.")
                } else if (!LabStatus.captureAdmission.canBegin()) {
                    startService(Intent(this@LabActivity, LabCaptureService::class.java).setAction("stop"))
                } else {
                    LabStatus.publish("Capture service is not running.")
                }
            }
        })
        muteSwitch = Switch(this).apply {
            text = "Mute sending"
            isEnabled = false
            isSaveEnabled = false
            setOnCheckedChangeListener { _, checked ->
                if (!updatingMute) {
                    startService(Intent(this@LabActivity, LabCaptureService::class.java)
                        .setAction(LabCaptureService.ACTION_SET_MUTED)
                        .putExtra(LabCaptureService.EXTRA_MUTED, checked))
                }
            }
        }
        layout.addView(muteSwitch)
        layout.addView(TextView(this).apply {
            text = "Already buffered audio may continue briefly after muting."
        })
        toneButton = Button(this).apply {
            text = if (tone == null) "Start test tone" else "Stop test tone"
            setOnClickListener {
                toggleTone()
                text = if (tone == null) "Start test tone" else "Stop test tone"
            }
        }
        layout.addView(toneButton)
        probeButton = Button(this).apply {
            text = "Play latency probe"
            setOnClickListener { toggleLatencyProbe() }
        }
        layout.addView(probeButton)
        retryButton = Button(this).apply {
            text = "Retry capture with current setup"
            visibility = View.GONE
            setOnClickListener {
                when (LabStatus.retryTransport) {
                    LabTransport.Adb -> startCapture(false)
                    LabTransport.Udp -> startCapture(true)
                    LabTransport.PairedUdp -> startCapture(true, paired = true)
                    null -> updateRetryButton()
                }
            }
        }
        layout.addView(retryButton)
        retryHint = TextView(this).apply { visibility = View.GONE }
        layout.addView(retryHint)
        signal = TextView(this)
        layout.addView(signal)
        duration = TextView(this)
        layout.addView(duration)
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
            val selected = LabCaptureUsage.fromValue(
                savedInstanceState?.getString("selectedUsage") ?: labPreferences.getString(PREF_USAGE, null),
            )
            setSelection(LabCaptureUsage.entries.indexOf(selected))
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(parent: AdapterView<*>?, view: android.view.View?, position: Int, id: Long) {
                    labPreferences.edit().putString(PREF_USAGE, LabCaptureUsage.entries.getOrElse(position) { LabCaptureUsage.All }.value).apply()
                }

                override fun onNothingSelected(parent: AdapterView<*>?) = Unit
            }
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
            val selected = OpusBitrate.fromBitsPerSecond(
                savedInstanceState?.getInt("selectedBitrate", defaultBitrate)
                    ?: labPreferences.getInt(PREF_BITRATE, defaultBitrate),
            )
            setSelection(OpusBitrate.entries.indexOf(selected))
            onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
                override fun onItemSelected(parent: AdapterView<*>?, view: android.view.View?, position: Int, id: Long) {
                    labPreferences.edit().putInt(PREF_BITRATE, OpusBitrate.entries.getOrElse(position) { OpusBitrate.STANDARD }.bitsPerSecond).apply()
                }

                override fun onNothingSelected(parent: AdapterView<*>?) = Unit
            }
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
        layout.addView(TextView(this).apply { text = "Desktop UDP port" })
        mediaPort = EditText(this).apply {
            contentDescription = "Desktop UDP port"
            keyListener = DigitsKeyListener.getInstance("0123456789")
            setSingleLine(true)
            setText(savedInstanceState?.getString("mediaPort")
                ?: labPreferences.getInt(PREF_MEDIA_PORT, DEFAULT_LAB_MEDIA_PORT).toString())
        }
        layout.addView(mediaPort)
        layout.addView(TextView(this).apply {
            text = "Lab invitation (optional; from the desktop listener)"
        })
        invitation = EditText(this).apply {
            hint = "Paste lab invitation"
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
            text = "Clear saved lab setup"
            setOnClickListener { clearSavedLabSetup() }
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
            setText(savedInstanceState?.getString("pin") ?: labPreferences.getString(PREF_PIN, ""))
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
            setText(savedInstanceState?.getString("host") ?: labPreferences.getString(PREF_HOST, "127.0.0.1"))
        }
        layout.addView(host)
        layout.addView(TextView(this).apply { text = "Paired control TCP port" })
        controlPort = EditText(this).apply {
            contentDescription = "Paired control TCP port"
            keyListener = DigitsKeyListener.getInstance("0123456789")
            setSingleLine(true)
            setText(savedInstanceState?.getString("controlPort") ?: labPreferences.getInt(PREF_CONTROL_PORT, DEFAULT_LAB_CONTROL_PORT).toString())
        }
        layout.addView(controlPort)
        layout.addView(Button(this).apply {
            text = "Use UDP address for control"
            setOnClickListener {
                val parsed = parseLabAddress(address.text.toString())
                if (parsed == null) {
                    showSetupFeedback("Enter a valid desktop IPv4 address for UDP.")
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
                    showSetupFeedback("Enter a non-loopback control IPv4 address before copying it to UDP.")
                } else {
                    address.setText(parsed.hostAddress)
                }
            }
        })
        layout.addView(TextView(this).apply {
            text = "New test tones use the current capture usage, or the selection when capture is stopped. Restart a playing tone to apply changes."
        })
        setContentView(ScrollView(this).apply { addView(layout) })
        handleIncomingInvitation(intent)
        if (savedInstanceState == null && importedInvitation != null && intent?.action != Intent.ACTION_SEND) {
            LabStatus.publishIfIdle("Restored the last lab setup. Check the UDP address before starting.")
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIncomingInvitation(intent)
    }

    override fun onStart() {
        super.onStart()
        LabStatus.observe(statusObserver)
        LabStatus.observeSignal(signalObserver)
        LabStatus.observeCaptureDuration(durationObserver)
        LabStatus.observeSenderQueueDrops(dropsObserver)
        LabStatus.observeSentPackets(sentPacketsObserver)
        LabStatus.observeEncoderRate(rateObserver)
        LabStatus.observeSessionConfig(sessionObserver)
        LabStatus.observeSenderMuted(muteObserver)
    }

    override fun onStop() {
        stopLatencyProbe()
        if (!isChangingConfigurations) {
            stopTone()
            toneButton.text = "Start test tone"
        }
        LabStatus.remove(statusObserver)
        LabStatus.removeSignal(signalObserver)
        LabStatus.removeCaptureDuration(durationObserver)
        LabStatus.removeSenderQueueDrops(dropsObserver)
        LabStatus.removeSentPackets(sentPacketsObserver)
        LabStatus.removeEncoderRate(rateObserver)
        LabStatus.removeSessionConfig(sessionObserver)
        LabStatus.removeSenderMuted(muteObserver)
        super.onStop()
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putBoolean("udpMode", udpMode)
        outState.putBoolean("pairedMode", pairedMode)
        outState.putString("pendingAddress", pendingAddress)
        outState.putInt("pendingMediaPort", pendingMediaPort)
        outState.putString("pendingPin", pendingPin)
        outState.putString("pendingHost", pendingHost)
        outState.putInt("pendingControlPort", pendingControlPort)
        outState.putString("pendingUsage", pendingUsage.value)
        outState.putString("selectedUsage", selectedUsage().value)
        outState.putInt("pendingBitrate", pendingBitrate.bitsPerSecond)
        outState.putInt("selectedBitrate", selectedBitrate().bitsPerSecond)
        importedInvitation?.let { outState.putString("importedInvitation", labHex(it)) }
        pendingInvitation?.let { outState.putByteArray("pendingInvitation", it.copyOf()) }
        outState.putString("address", address.text.toString())
        outState.putString("mediaPort", mediaPort.text.toString())
        outState.putString("invitation", invitation.text.toString())
        outState.putString("setupFeedback", setupFeedback.text.toString())
        outState.putString("pin", pin.text.toString())
        outState.putString("host", host.text.toString())
        outState.putString("controlPort", controlPort.text.toString())
        super.onSaveInstanceState(outState)
    }

    private fun startCapture(udp: Boolean, paired: Boolean = false) {
        if (!LabStatus.captureAdmission.canBegin()) {
            showCaptureBusy()
            return
        }
        var replacedInvitation = false
        if (udp) {
            val parsed = parseLabAddress(address.text.toString())
            if (parsed == null) {
                LabStatus.publish("Enter a valid desktop IPv4 address for UDP.")
                return
            }
            val port = parseLabMediaPort(mediaPort.text.toString())
            if (port == null) {
                LabStatus.publish("Enter a desktop UDP port from 1 to 65535.")
                return
            }
            pendingAddress = requireNotNull(parsed.hostAddress)
            pendingMediaPort = port
            labPreferences.edit().putString("desktopAddress", pendingAddress).putInt(PREF_MEDIA_PORT, port).apply()
        }
        if (paired) {
            val enteredHost = host.text.toString().trim()
            val parsedHost = if (enteredHost == "127.0.0.1") enteredHost else parseLabAddress(enteredHost)?.hostAddress
            if (parsedHost == null) {
                LabStatus.publish("Enter a valid paired control IPv4 address.")
                return
            }
            val parsedControlPort = parseLabMediaPort(controlPort.text.toString())
            if (parsedControlPort == null) {
                LabStatus.publish("Enter a paired control TCP port from 1 to 65535.")
                return
            }
            val entered = pin.text.toString().trim()
            if (runCatching { labInvitation(entered, parsedHost, parsedControlPort).fill(0) }.isFailure) {
                LabStatus.publish("Paste the desktop listener's complete SPKI_HEX.")
                return
            }
            if (importedInvitation != null && (!entered.equals(importedPin, ignoreCase = true) || parsedHost != importedHost || parsedControlPort != importedControlPort)) {
                importedInvitation?.fill(0)
                importedInvitation = null
                labPreferences.edit().remove(PREF_INVITATION).apply()
                replacedInvitation = true
            }
            pendingPin = entered
            pendingHost = parsedHost
            pendingControlPort = parsedControlPort
            labPreferences.edit()
                .putString(PREF_PIN, entered)
                .putString(PREF_HOST, parsedHost)
                .putInt(PREF_CONTROL_PORT, parsedControlPort)
                .apply()
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
        clearPendingInvitation()
        pendingInvitation = if (paired) importedInvitation?.copyOf() else null
        setupFeedback.text = if (replacedInvitation) {
            "Connection settings changed; using the manual lab setup instead of the imported invitation."
        } else ""
        retryButton.visibility = View.GONE
        retryHint.visibility = View.GONE
        if (needsPermission) {
            try {
                requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 1)
            } catch (_: RuntimeException) {
                LabStatus.captureAdmission.launchFailed()
                clearPendingInvitation()
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

    private fun clearPendingInvitation() {
        pendingInvitation?.fill(0)
        pendingInvitation = null
    }

    private fun showSetupFeedback(message: String) {
        if (LabStatus.captureAdmission.canBegin()) {
            setupFeedback.text = ""
            LabStatus.publish(message)
        } else {
            setupFeedback.text = "Next setup: $message"
        }
    }

    private fun updateRetryButton() {
        if (!::retryButton.isInitialized) return
        val transport = LabStatus.retryTransport
        val retryable = LabStatus.captureAdmission.canBegin() && transport != null
        retryButton.visibility = if (retryable) View.VISIBLE else View.GONE
        retryHint.visibility = retryButton.visibility
        retryHint.text = if (retryable) "Retry transport: ${transport?.label}. Current settings will be used." else ""
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
            text.isNullOrEmpty() -> showSetupFeedback(emptyMessage)
            else -> {
                try {
                    parseLabInvitationText(text).payload.fill(0)
                    invitation.setText(text)
                    showSetupFeedback(successMessage)
                } catch (_: IllegalArgumentException) {
                    showSetupFeedback("Paste a valid lab invitation payload.")
                }
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
            showSetupFeedback("Copy the desktop SPKI_HEX first.")
            return
        }
        try {
            labInvitation(text).fill(0)
            pin.setText(text)
            showSetupFeedback("Desktop SPKI pasted. Enter the control address and start paired capture.")
        } catch (_: IllegalArgumentException) {
            showSetupFeedback("The copied desktop SPKI_HEX is invalid.")
        }
    }

    private fun applyInvitation(useAddressForUdp: Boolean = false): Boolean {
        var raw: ByteArray? = null
        var retained = false
        try {
            val parsed = parseLabInvitationText(invitation.text.toString())
            raw = parsed.payload
            val decoded = PairingQr.decode(requireNotNull(raw))
            try {
                val udpAddress = parseLabAddress(decoded.controlHost)
                require(decoded.controlHost == "127.0.0.1" || udpAddress != null)
                val decodedPin = labHex(decoded.desktopTlsSpki)
                pin.setText(decodedPin)
                host.setText(decoded.controlHost)
                controlPort.setText(decoded.controlPort.toString())
                parsed.mediaPort?.let { port ->
                    mediaPort.setText(port.toString())
                    labPreferences.edit().putInt(PREF_MEDIA_PORT, port).apply()
                }
                if (useAddressForUdp && udpAddress != null) {
                    address.setText(udpAddress.hostAddress)
                    labPreferences.edit().putString("desktopAddress", udpAddress.hostAddress).apply()
                }
                importedInvitation?.fill(0)
                importedInvitation = raw
                importedPin = decodedPin
                importedHost = decoded.controlHost
                importedControlPort = decoded.controlPort
                labPreferences.edit()
                    .putString(PREF_INVITATION, labHex(raw))
                    .putString(PREF_PIN, decodedPin)
                    .putString(PREF_HOST, decoded.controlHost)
                    .putInt(PREF_CONTROL_PORT, decoded.controlPort)
                    .apply()
                retained = true
                invitation.setText("")
                val portMessage = parsed.mediaPort?.let { " UDP port set to $it." } ?: ""
                val addressMessage = if (useAddressForUdp && udpAddress != null) " UDP uses the same address."
                    else " Check the desktop IPv4 for UDP separately."
                val controlMessage = if (decoded.controlPort != DEFAULT_LAB_CONTROL_PORT) " Control TCP port ${decoded.controlPort}." else ""
                showSetupFeedback("Invitation applied.$portMessage$addressMessage$controlMessage")
            } finally {
                decoded.pairingId.fill(0)
                decoded.pairingSecret.fill(0)
                decoded.desktopTlsSpki.fill(0)
            }
        } catch (_: IllegalArgumentException) {
            showSetupFeedback("Paste a valid lab invitation payload.")
        } finally {
            if (!retained) raw?.fill(0)
        }
        return retained
    }

    private fun clearSavedLabSetup() {
        importedInvitation?.fill(0)
        importedInvitation = null
        importedPin = ""
        importedHost = ""
        importedControlPort = DEFAULT_LAB_CONTROL_PORT
        labPreferences.edit()
            .remove(PREF_INVITATION)
            .remove(PREF_PIN)
            .remove(PREF_HOST)
            .remove(PREF_CONTROL_PORT)
            .remove("desktopAddress")
            .remove(PREF_MEDIA_PORT)
            .apply()
        invitation.setText("")
        pin.setText("")
        host.setText("127.0.0.1")
        controlPort.setText(DEFAULT_LAB_CONTROL_PORT.toString())
        address.setText(DEFAULT_LAB_ADDRESS)
        mediaPort.setText(DEFAULT_LAB_MEDIA_PORT.toString())
        showSetupFeedback("Saved lab setup cleared.")
    }

    internal fun applyScannedInvitation(contents: String?, startAfterScan: Boolean = false) {
        if (contents == null || contents.length > MAX_LAB_INVITATION_TEXT) {
            showSetupFeedback("Scan a valid lab invitation, or paste its payload.")
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
            val cancelled = LabStatus.captureAdmission.cancellationPending()
            if (!LabStatus.captureAdmission.permissionResult(granted)) {
                if (cancelled && LabStatus.captureAdmission.canBegin()) LabStatus.publish("Capture start cancelled.")
                return
            }
            if (granted) {
                if (LabStatus.captureAdmission.awaitingNotification()) requestNotificationPermission() else requestProjection()
            }
            else {
                clearPendingInvitation()
                LabStatus.publish("Microphone permission is required for playback capture.")
            }
        } else if (requestCode == 5) {
            val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
            val cancelled = LabStatus.captureAdmission.cancellationPending()
            if (!LabStatus.captureAdmission.notificationResult(granted)) {
                if (cancelled && LabStatus.captureAdmission.canBegin()) LabStatus.publish("Capture start cancelled.")
                return
            }
            if (granted) requestProjection()
            else {
                clearPendingInvitation()
                LabStatus.publish("Notification permission is required to show capture controls. Enable it in app settings and try again.")
            }
        }
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        try {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 5)
        } catch (_: RuntimeException) {
            LabStatus.captureAdmission.launchFailed()
            clearPendingInvitation()
            LabStatus.publish("Unable to request notification permission. Try again.")
        }
    }

    private fun requestProjection() {
        try {
            val manager = getSystemService(MediaProjectionManager::class.java)
            startActivityForResult(manager.createScreenCaptureIntent(), 2)
        } catch (_: RuntimeException) {
            LabStatus.captureAdmission.launchFailed()
            clearPendingInvitation()
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
            val cancelled = LabStatus.captureAdmission.cancellationPending()
            if (!LabStatus.captureAdmission.projectionResult(granted)) {
                if (cancelled && LabStatus.captureAdmission.canBegin()) LabStatus.publish("Capture start cancelled.")
                return
            }
            if (!granted) {
                clearPendingInvitation()
                LabStatus.publish("Screen capture consent was not granted.")
                return
            }
            try {
                startForegroundService(Intent(this, LabCaptureService::class.java).apply {
                    putExtra("resultCode", resultCode)
                    putExtra("resultData", data)
                    putExtra("udpMode", udpMode)
                    putExtra("udpAddress", pendingAddress)
                    putExtra("udpPort", pendingMediaPort)
                    putExtra("pairedMode", pairedMode)
                    putExtra("captureUsage", pendingUsage.value)
                    putExtra("bitrate", pendingBitrate.bitsPerSecond)
                    if (pairedMode) {
                        putExtra("pairedPin", pendingPin)
                        putExtra("pairedHost", pendingHost)
                        putExtra("pairedControlPort", pendingControlPort)
                        pendingInvitation?.let { putExtra("pairedInvitation", it.copyOf()) }
                    }
                })
            } catch (_: RuntimeException) {
                LabStatus.captureAdmission.launchFailed()
                LabStatus.publish("Unable to start capture. Try again.")
            } finally {
                clearPendingInvitation()
            }
        }
    }

    private fun toggleTone() {
        if (tone != null) {
            stopTone()
            return
        }
        stopLatencyProbe()
        val usage = (LabStatus.captureUsage ?: selectedUsage()).usages.first()
        for (format in testToneFormats) {
            val samples = testToneSamples(format.sampleRate, format.channels)
            var track: AudioTrack? = null
            try {
                track = AudioTrack.Builder()
                    .setAudioAttributes(AudioAttributes.Builder().setUsage(usage).build())
                    .setAudioFormat(AudioFormat.Builder().setSampleRate(format.sampleRate)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(format.channelMask).build())
                    .setTransferMode(AudioTrack.MODE_STATIC)
                    .setBufferSizeInBytes(samples.size * Short.SIZE_BYTES)
                    .build()
                check(track.write(samples, 0, samples.size) == samples.size)
                check(track.setLoopPoints(0, format.sampleRate, -1) == AudioTrack.SUCCESS)
                track.play()
                tone = track
                return
            } catch (_: RuntimeException) {
                track?.release()
            }
        }
        LabStatus.publish("Unable to play test tone on this device.")
    }

    override fun onRetainNonConfigurationInstance(): Any? = tone

    private fun toggleLatencyProbe() {
        if (latencyProbe != null) {
            stopLatencyProbe()
            return
        }
        val captureUsage = LabStatus.captureUsage
        if (captureUsage == null) {
            Toast.makeText(this, "Start capture before playing the latency probe.", Toast.LENGTH_SHORT).show()
            return
        }
        stopTone()
        toneButton.text = "Start test tone"
        val samples = latencyProbeSamples()
        var track: AudioTrack? = null
        try {
            track = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(captureUsage.usages.first()).build())
                .setAudioFormat(AudioFormat.Builder().setSampleRate(LATENCY_PROBE_RATE)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                .setTransferMode(AudioTrack.MODE_STATIC)
                .setBufferSizeInBytes(samples.size * Short.SIZE_BYTES)
                .build()
            check(track.write(samples, 0, samples.size) == samples.size)
            check(track.setNotificationMarkerPosition(samples.size) == AudioTrack.SUCCESS)
            track.setPlaybackPositionUpdateListener(object : AudioTrack.OnPlaybackPositionUpdateListener {
                override fun onMarkerReached(completed: AudioTrack) {
                    if (latencyProbe === completed) stopLatencyProbe()
                }
                override fun onPeriodicNotification(track: AudioTrack) = Unit
            }, probeHandler)
            track.play()
            check(track.playState == AudioTrack.PLAYSTATE_PLAYING)
            latencyProbe = track
            probeButton.text = "Stop latency probe"
            val timeout = Runnable { stopLatencyProbe() }
            probeTimeout = timeout
            probeHandler.postDelayed(timeout, 6_000)
        } catch (_: RuntimeException) {
            track?.release()
            Toast.makeText(this, "Unable to play the 48 kHz latency probe on this device.", Toast.LENGTH_SHORT).show()
        }
    }

    private fun stopLatencyProbe() {
        probeTimeout?.let(probeHandler::removeCallbacks)
        probeTimeout = null
        latencyProbe?.let {
            latencyProbe = null
            try { it.stop() } finally { it.release() }
        }
        if (::probeButton.isInitialized) probeButton.text = "Play latency probe"
    }

    private fun stopTone() {
        tone?.let {
            try {
                it.stop()
            } finally {
                it.release()
                tone = null
            }
        }
    }

    internal fun isTestTonePlaying(): Boolean = tone?.playState == AudioTrack.PLAYSTATE_PLAYING

    internal fun testToneUsage(): Int? = tone?.audioAttributes?.usage

    override fun onDestroy() {
        stopLatencyProbe()
        importedInvitation?.fill(0)
        importedInvitation = null
        clearPendingInvitation()
        if (!isChangingConfigurations) stopTone()
        tone = null
        super.onDestroy()
    }
}

internal fun buildLabEvidenceShareIntent(evidence: String): Intent =
    Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_TEXT, evidence)
    }

private const val PREF_INVITATION = "labInvitation"
private const val PREF_PIN = "labPin"
private const val PREF_HOST = "labHost"
private const val PREF_CONTROL_PORT = "labControlPort"
private const val PREF_USAGE = "labCaptureUsage"
private const val PREF_BITRATE = "labBitrate"
private const val PREF_MEDIA_PORT = "labMediaPort"
