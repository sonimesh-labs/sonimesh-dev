package com.sonimesh.opusspike

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.projection.MediaProjectionManager
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
import com.sonimesh.sender.protocol.PairingQr
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
    private var importedInvitation: ByteArray? = null
    private var importedPin = ""
    private var importedHost = ""
    private lateinit var address: EditText
    private lateinit var invitation: EditText
    private lateinit var pin: EditText
    private lateinit var host: EditText
    private lateinit var status: TextView
    private lateinit var usage: Spinner
    private val statusObserver: (String) -> Unit = { status.text = "Status: $it" }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        udpMode = savedInstanceState?.getBoolean("udpMode") ?: false
        pairedMode = savedInstanceState?.getBoolean("pairedMode") ?: false
        pendingAddress = savedInstanceState?.getString("pendingAddress") ?: DEFAULT_LAB_ADDRESS
        pendingPin = savedInstanceState?.getString("pendingPin") ?: ""
        pendingHost = savedInstanceState?.getString("pendingHost") ?: "127.0.0.1"
        pendingUsage = LabCaptureUsage.fromValue(savedInstanceState?.getString("pendingUsage"))
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
        }
        layout.addView(invitation)
        layout.addView(Button(this).apply {
            text = "Scan lab invitation"
            setOnClickListener {
                try {
                    startActivityForResult(Intent("com.google.zxing.client.android.SCAN").apply {
                        putExtra("SCAN_MODE", "QR_CODE_MODE")
                    }, 3)
                } catch (_: ActivityNotFoundException) {
                    LabStatus.publish("No compatible scanner found. Paste the lab invitation instead.")
                }
            }
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
            setOnClickListener { stopService(Intent(this@LabActivity, LabCaptureService::class.java)) }
        })
        layout.addView(Button(this).apply {
            text = "Toggle test tone"
            setOnClickListener { toggleTone() }
        })
        setContentView(ScrollView(this).apply { addView(layout) })
    }

    override fun onStart() {
        super.onStart()
        LabStatus.observe(statusObserver)
    }

    override fun onStop() {
        LabStatus.remove(statusObserver)
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
        importedInvitation?.let { outState.putString("importedInvitation", labHex(it)) }
        outState.putString("importedPin", importedPin)
        outState.putString("importedHost", importedHost)
        outState.putString("address", address.text.toString())
        outState.putString("pin", pin.text.toString())
        outState.putString("host", host.text.toString())
        super.onSaveInstanceState(outState)
    }

    private fun startCapture(udp: Boolean, paired: Boolean = false) {
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
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), 1)
        } else requestProjection()
    }

    private fun selectedUsage(): LabCaptureUsage =
        LabCaptureUsage.entries.getOrElse(usage.selectedItemPosition) { LabCaptureUsage.All }

    private fun applyInvitation(useAddressForUdp: Boolean = false) {
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
    }

    internal fun applyScannedInvitation(contents: String?) {
        if (contents == null || contents.length > PairingQr.MAX_PAYLOAD_BYTES * 2) {
            LabStatus.publish("Scan a valid lab invitation, or paste its payload.")
            return
        }
        invitation.setText(contents)
        applyInvitation(useAddressForUdp = true)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == 1) {
            if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) requestProjection()
            else LabStatus.publish("Microphone permission is required for playback capture.")
        }
    }

    private fun requestProjection() {
        val manager = getSystemService(MediaProjectionManager::class.java)
        startActivityForResult(manager.createScreenCaptureIntent(), 2)
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == 3) {
            if (resultCode == RESULT_OK) applyScannedInvitation(data?.getStringExtra("SCAN_RESULT"))
            return
        }
        if (requestCode == 2 && resultCode == RESULT_OK && data != null) {
            startForegroundService(Intent(this, LabCaptureService::class.java).apply {
                putExtra("resultCode", resultCode)
                putExtra("resultData", data)
                putExtra("udpMode", udpMode)
                putExtra("udpAddress", pendingAddress)
                putExtra("pairedMode", pairedMode)
                putExtra("captureUsage", pendingUsage.value)
                if (pairedMode) {
                    putExtra("pairedPin", pendingPin)
                    putExtra("pairedHost", pendingHost)
                    importedInvitation?.let { putExtra("pairedInvitation", it.copyOf()) }
                }
            })
            importedInvitation?.fill(0)
            importedInvitation = null
        } else if (requestCode == 2) {
            LabStatus.publish("Screen capture consent was not granted.")
        }
    }

    private fun toggleTone() {
        tone?.let { it.stop(); it.release(); tone = null; return }
        val samples = ShortArray(48_000 * 2) { index ->
            (sin(2.0 * PI * (index / 2 % 48) / 48.0) * 3_000).toInt().toShort()
        }
        val track = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build())
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

    override fun onDestroy() {
        importedInvitation?.fill(0)
        importedInvitation = null
        tone?.let { it.stop(); it.release() }
        tone = null
        super.onDestroy()
    }
}
