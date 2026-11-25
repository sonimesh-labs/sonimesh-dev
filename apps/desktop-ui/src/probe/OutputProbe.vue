<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import QRCode from 'qrcode'

interface OutputDevice {
  label: string
  sampleRateHz: number | null
  channels: number | null
  isDefault: boolean
}

declare global {
  interface Window {
    __TAURI__?: { core: { invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> } }
  }
}

const devices = ref<OutputDevice[]>([])
const busy = ref(false)
const error = ref<string | null>(null)
const loaded = ref(false)
const generation = ref(0)
const playing = ref<number | null>(null)
const message = ref<string | null>(null)
interface LabMetrics {
  packets: string; rejected: string; decoded: string; peak: string
  plc: string; decodeErrors: string; buffered: string; output: 'active' | 'recovering' | 'software-check' | 'stopped'
  late: string | null; resyncDropped: string | null
  media?: 'waiting' | 'receiving' | 'interrupted' | null
  playout: 'fixed' | 'adaptive' | null
  adaptive: { target: number; min: number; max: number; jitterMs: number; gated: string; forced: string; resets: string } | null
}
interface LabSnapshot { running: boolean; sessionActive: boolean; status: string; invitation: string | null; events?: string[]; metrics: LabMetrics | null; signal?: { decoded: number; peak: number | null } | null; signalStale?: boolean; volume: number | null; muted: boolean | null; mediaPort?: number | null }
const lab = ref<LabSnapshot>({ running: false, sessionActive: false, status: 'Checking listener…', invitation: null, metrics: null, volume: null, muted: null })
const invitationQr = ref<string | null>(null)
const invitationQrError = ref(false)
const invitationCopyMessage = ref<string | null>(null)
const includeDirectPort = ref(false)
watch(() => lab.value.running, (running) => { if (!running) includeDirectPort.value = false })
const displayedInvitation = computed(() => {
  const invitation = lab.value.invitation
  if (!invitation) return null
  return includeDirectPort.value && lab.value.running && lab.value.mediaPort != null
    ? `SONIMESH-LAB1:${lab.value.mediaPort}:${invitation}` : invitation
})
watch(displayedInvitation, async (invitation, _previous, onCleanup) => {
  let cancelled = false
  onCleanup(() => { cancelled = true })
  invitationQr.value = null
  invitationQrError.value = false
  invitationCopyMessage.value = null
  if (!invitation) return
  try {
    const svg = await QRCode.toString(invitation.toUpperCase(), { type: 'svg', errorCorrectionLevel: 'M', margin: 4 })
    if (!cancelled) invitationQr.value = `data:image/svg+xml,${encodeURIComponent(svg)}`
  } catch {
    if (!cancelled) invitationQrError.value = true
  }
})

async function copyInvitation() {
  const invitation = displayedInvitation.value
  if (!invitation) return
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(invitation)
    invitationCopyMessage.value = 'Invitation copied.'
  } catch {
    invitationCopyMessage.value = 'Could not copy the invitation. Select the payload below instead.'
  }
}

async function shareInvitation() {
  const invitation = displayedInvitation.value
  if (!invitation) return
  try {
    if (navigator.share) {
      await navigator.share({ title: 'SoniMesh lab invitation', text: invitation })
      invitationCopyMessage.value = 'Invitation shared.'
      return
    }
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(invitation)
    invitationCopyMessage.value = 'System sharing is unavailable; invitation copied instead.'
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'AbortError') return
    invitationCopyMessage.value = 'Could not share the invitation. Select the payload below instead.'
  }
}

function evidenceValue(value: unknown): string {
  const text = value == null ? '-' : String(value)
  return text.length <= 80 ? text : `${text.slice(0, 77)}...`
}

const LAB_SETTINGS_KEY = 'sonimesh.outputProbe.labSettings'
const startupBufferChoices = [20, 40, 60, 80, 100, 120]

interface StoredLabSettings {
  labHost?: string
  androidHost?: string
  labOutput?: string
  labOutputDevice?: { label: string; sampleRateHz: number; channels: number }
  startupBufferMs?: number
  adaptive?: boolean
  mediaPort?: number
}

function readLabSettings(): StoredLabSettings {
  try {
    const raw = localStorage.getItem(LAB_SETTINGS_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as Record<string, unknown>
    return {
      labHost: typeof parsed.labHost === 'string' && parsed.labHost.length <= 64 ? parsed.labHost : undefined,
      androidHost: typeof parsed.androidHost === 'string' && parsed.androidHost.length <= 64 ? parsed.androidHost : undefined,
      labOutput: parsed.labOutput === '' || parsed.labOutput === 'unavailable' || (typeof parsed.labOutput === 'string' && /^\d+$/.test(parsed.labOutput)) ? parsed.labOutput as string : undefined,
      labOutputDevice: parsed.labOutputDevice && typeof parsed.labOutputDevice === 'object' &&
        typeof (parsed.labOutputDevice as Record<string, unknown>).label === 'string' &&
        typeof (parsed.labOutputDevice as Record<string, unknown>).sampleRateHz === 'number' &&
        typeof (parsed.labOutputDevice as Record<string, unknown>).channels === 'number'
        ? parsed.labOutputDevice as StoredLabSettings['labOutputDevice'] : undefined,
      startupBufferMs: typeof parsed.startupBufferMs === 'number' && startupBufferChoices.includes(parsed.startupBufferMs) ? parsed.startupBufferMs : undefined,
      adaptive: typeof parsed.adaptive === 'boolean' ? parsed.adaptive : undefined,
      mediaPort: typeof parsed.mediaPort === 'number' && Number.isInteger(parsed.mediaPort) && parsed.mediaPort >= 1 && parsed.mediaPort <= 65535 ? parsed.mediaPort : undefined,
    }
  } catch {
    return {}
  }
}

function writeLabSettings(settings: StoredLabSettings) {
  try {
    localStorage.setItem(LAB_SETTINGS_KEY, JSON.stringify(settings))
  } catch {
    // Browser storage is a convenience; the probe remains usable when unavailable.
  }
}

function clearLabSettings() {
  try {
    localStorage.removeItem(LAB_SETTINGS_KEY)
  } catch {
    // Browser storage is a convenience; resetting the visible fields still works.
  }
  labHost.value = '127.0.0.1'
  androidHost.value = ''
  labOutput.value = ''
  labOutputDevice.value = null
  startupBufferMs.value = 60
  adaptive.value = false
  mediaPort.value = '39093'
  invalidateHostSuggestion()
}

const storedLabSettings = readLabSettings()
const labOutputDevice = ref(storedLabSettings.labOutput === '' ? null : storedLabSettings.labOutputDevice ?? null)

const labEvidenceMessage = ref<string | null>(null)

function labEvidenceText(): string | null {
  const metrics = lab.value.metrics
  const events = lab.value.events ?? []
  const signal = lab.value.sessionActive ? lab.value.signal : null
  if (!metrics && !signal && events.length === 0) return null
  const lines = [
    'SoniMesh paired audio lab',
    `Status: ${evidenceValue(lab.value.status)}`,
    `Session: ${lab.value.sessionActive ? 'active' : 'last reported'}`,
  ]
  if (signal) {
    lines.push(`Last decoded interval${lab.value.signalStale ? ' (stale: no update for at least 3 seconds)' : ''}: ${signal.decoded} frames${signal.peak === null ? '; no peak' : `; peak ${signal.peak.toFixed(6)}`} (before output gain; excludes concealment)`)
  }
  if (metrics) {
    lines.push(`Accepted packets: ${evidenceValue(metrics.packets)}`)
    lines.push(`Packets rejected: ${evidenceValue(metrics.rejected)}`)
    lines.push(`Decoded frames: ${evidenceValue(metrics.decoded)}`)
    lines.push(`Decoded peak: ${evidenceValue(metrics.peak)}`)
    lines.push(`Concealed frames: ${evidenceValue(metrics.plc)}`)
    lines.push(`Decode errors: ${evidenceValue(metrics.decodeErrors)}`)
    lines.push(`Buffered frames: ${evidenceValue(metrics.buffered)}`)
    lines.push(`Output: ${evidenceValue(metrics.output)}`)
    if (metrics.media) lines.push(`Media arrival: ${evidenceValue(metrics.media)}`)
    if (metrics.late != null) lines.push(`Late packets: ${evidenceValue(metrics.late)}`)
    if (metrics.resyncDropped != null) lines.push(`Frames dropped on resync: ${evidenceValue(metrics.resyncDropped)}`)
    if (metrics.playout) lines.push(`Playout mode: ${evidenceValue(metrics.playout)}`)
    if (metrics.adaptive) {
      lines.push(`Adaptive target: ${evidenceValue(metrics.adaptive.target)}`)
      lines.push(`Adaptive jitter: ${evidenceValue(metrics.adaptive.jitterMs)} ms`)
      lines.push(`Gated ticks: ${evidenceValue(metrics.adaptive.gated)}`)
      lines.push(`Forced playouts: ${evidenceValue(metrics.adaptive.forced)}`)
      lines.push(`Stream resets: ${evidenceValue(metrics.adaptive.resets)}`)
    }
  }
  if (events.length) {
    lines.push('Recent listener events:')
    for (const event of events) lines.push(`- ${evidenceValue(event)}`)
  }
  return lines.join('\n')
}

async function copyLabEvidence() {
  const text = labEvidenceText()
  if (!text) return
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(text)
    labEvidenceMessage.value = 'Session evidence copied.'
  } catch {
    labEvidenceMessage.value = 'Could not copy session evidence. Select the visible metrics instead.'
  }
}

async function shareLabEvidence() {
  const text = labEvidenceText()
  if (!text) return
  try {
    if (navigator.share) {
      await navigator.share({ title: 'SoniMesh session evidence', text })
      labEvidenceMessage.value = 'Session evidence shared.'
      return
    }
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(text)
    labEvidenceMessage.value = 'System sharing is unavailable; session evidence copied instead.'
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'AbortError') return
    labEvidenceMessage.value = 'Could not share session evidence. Select the visible metrics instead.'
  }
}
const labHost = ref(storedLabSettings.labHost ?? '127.0.0.1')
const androidHost = ref(storedLabSettings.androidHost ?? '')
const suggestedHost = ref<string | null>(null)
const suggestError = ref<string | null>(null)
const suggestBusy = ref(false)
const labOutput = ref(storedLabSettings.labOutput ?? '')
const startupBufferMs = ref(storedLabSettings.startupBufferMs ?? 60)
const adaptive = ref(storedLabSettings.adaptive ?? false)
const mediaPort = ref(String(storedLabSettings.mediaPort ?? 39093))
const mediaPortValid = computed(() => /^\d{1,5}$/.test(mediaPort.value) && Number(mediaPort.value) >= 1 && Number(mediaPort.value) <= 65535)
const labBusy = ref(true)
let labKnown = false
const restoredLab = ref(false)
const labError = ref<string | null>(null)
const controlError = ref<string | null>(null)
const controlBusy = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let labRequest = 0
let reconciling = false
let controlRequest = 0
let controlTimer: ReturnType<typeof setTimeout> | undefined
let pendingControl: { volume: number | null; muted: boolean | null; sent: boolean } | null = null
let suggestRequest = 0

watch([labHost, androidHost, labOutput, startupBufferMs, adaptive, mediaPort], () => {
  if (labOutput.value === '') labOutputDevice.value = null
  const selected = !loaded.value || labOutput.value === '' || labOutput.value === 'unavailable'
    ? undefined
    : devices.value[Number(labOutput.value)]
  if (selected?.sampleRateHz != null && selected.channels != null) {
    labOutputDevice.value = {
      label: selected.label,
      sampleRateHz: selected.sampleRateHz,
      channels: selected.channels,
    }
  }
  writeLabSettings({
    labHost: labHost.value,
    androidHost: androidHost.value,
    labOutput: labOutput.value,
    labOutputDevice: labOutputDevice.value ?? undefined,
    startupBufferMs: startupBufferMs.value,
    adaptive: adaptive.value,
    mediaPort: mediaPortValid.value ? Number(mediaPort.value) : undefined,
  })
})

watch(labOutputDevice, (device) => {
  const settings = readLabSettings()
  writeLabSettings({ ...settings, labOutputDevice: device ?? undefined })
})

function clearControl() {
  controlRequest++
  clearTimeout(controlTimer)
  pendingControl = null
  controlBusy.value = false
}

function confirmControl(snapshot: LabSnapshot) {
  if (!snapshot.running) {
    clearControl()
    return
  }
  if (pendingControl?.sent &&
      (pendingControl.volume === null || pendingControl.volume === snapshot.volume) &&
      (pendingControl.muted === null || pendingControl.muted === snapshot.muted)) clearControl()
}

function invalidateHostSuggestion() {
  suggestRequest++
  suggestBusy.value = false
  suggestedHost.value = null
  suggestError.value = null
}

async function suggestHost() {
  if (!window.__TAURI__ || suggestBusy.value) return
  const request = ++suggestRequest
  suggestBusy.value = true
  suggestedHost.value = null
  suggestError.value = null
  try {
    const result = await window.__TAURI__.core.invoke<string>('suggest_lab_host', { androidHost: androidHost.value.trim() })
    if (!disposed && request === suggestRequest) suggestedHost.value = result
  } catch (cause) {
    if (!disposed && request === suggestRequest) suggestError.value = typeof cause === 'string' ? cause : 'Could not suggest a desktop address.'
  } finally {
    if (!disposed && request === suggestRequest) suggestBusy.value = false
  }
}

async function pollLab(retryUnknown = false) {
  const request = labRequest
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    const snapshot = await window.__TAURI__.core.invoke<LabSnapshot>('lab_status')
    if (disposed || request !== labRequest) return
    const wasRunning = lab.value.running
    const wasSessionActive = lab.value.sessionActive
    const hadMetrics = lab.value.metrics !== null
    lab.value = snapshot
    if ((snapshot.sessionActive && !wasSessionActive) || (snapshot.metrics === null && hadMetrics)) {
      labEvidenceMessage.value = null
    }
    confirmControl(snapshot)
    labError.value = null
    if (reconciling) {
      reconciling = false
      labBusy.value = false
    }
    if (!labKnown) {
      labKnown = true
      restoredLab.value = snapshot.running
      labBusy.value = false
      if (!snapshot.running) void refresh()
    } else if (wasRunning && !snapshot.running) {
      restoredLab.value = false
      if (!loaded.value) void refresh()
    }
    if (!disposed && lab.value.running) timer = setTimeout(pollLab, 1000)
  } catch {
    if (!disposed && request === labRequest) {
      labError.value = 'Could not check the listener. Retrying…'
      if (retryUnknown || !labKnown || lab.value.running) timer = setTimeout(() => pollLab(retryUnknown), 1000)
    }
  }
}

async function toggleLab() {
  if (labBusy.value || !window.__TAURI__ || (!lab.value.running && (playing.value !== null || busy.value || labOutput.value === 'unavailable' || !mediaPortValid.value))) return
  labBusy.value = true
  labError.value = null
  clearControl()
  controlError.value = null
  const starting = !lab.value.running
  labRequest++
  clearTimeout(timer)
  try {
    lab.value = lab.value.running
      ? await window.__TAURI__.core.invoke<LabSnapshot>('stop_lab')
      : await window.__TAURI__.core.invoke<LabSnapshot>('start_lab', { host: labHost.value.trim(), generation: generation.value, index: labOutput.value === '' ? null : Number(labOutput.value), startupBufferMs: startupBufferMs.value, adaptive: adaptive.value, mediaPort: Number(mediaPort.value) })
    if (starting) labEvidenceMessage.value = null
    restoredLab.value = false
    if (!disposed && lab.value.running) timer = setTimeout(pollLab, 1000)
  } catch (cause) {
    reconciling = true
    await pollLab(true)
    if (!reconciling) labError.value = typeof cause === 'string' ? cause : 'Could not change listener state.'
  } finally {
    if (!reconciling) labBusy.value = false
    if (!disposed && labKnown && !lab.value.running && !loaded.value) void refresh()
  }
}

async function controlLab(volume: number | null, muted: boolean | null) {
  if (!window.__TAURI__ || controlBusy.value || labBusy.value || !lab.value.running) return
  controlBusy.value = true
  controlError.value = null
  const request = ++controlRequest
  pendingControl = { volume, muted, sent: false }
  controlTimer = setTimeout(() => {
    if (request !== controlRequest || disposed) return
    clearControl()
    controlError.value = 'Listener did not confirm the adjustment. Check the displayed value and try again.'
  }, 5000)
  try {
    await window.__TAURI__.core.invoke('control_lab', { volume, muted })
    if (disposed || request !== controlRequest || !pendingControl) return
    pendingControl.sent = true
    confirmControl(lab.value)
  } catch (cause) {
    if (disposed || request !== controlRequest) return
    clearControl()
    controlError.value = typeof cause === 'string' ? cause : 'Could not change audio control.'
  }
}

function changeLabVolume(event: Event) {
  const input = event.target as HTMLInputElement
  const volume = Number(input.value)
  input.value = String(lab.value.volume)
  void controlLab(volume, null)
}
const playbackErrors = new Set([
  'Could not select output.',
  'Device list changed. Refresh and try again.',
  'Device is no longer available.',
  'A test tone is already playing.',
  'Stop the listener before testing an output.',
  'Could not inspect this output.',
  'This output format is not supported.',
  'Could not start this output.',
  'Output stopped or reported a playback error.',
])

async function refresh() {
  if (busy.value || lab.value.running || labBusy.value) return
  busy.value = true
  error.value = null
  message.value = null
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    const selectedIndex = loaded.value && labOutput.value !== '' && labOutput.value !== 'unavailable' ? Number(labOutput.value) : null
    const snapshot = await window.__TAURI__.core.invoke<{ generation: number; devices: OutputDevice[]; selectedIndex?: number | null }>('probe_outputs', { generation: generation.value, selectedIndex })
    generation.value = snapshot.generation
    devices.value = snapshot.devices
    if (!loaded.value && labOutputDevice.value) {
      const matches = snapshot.devices.flatMap((device, index) =>
        device.label === labOutputDevice.value?.label &&
        device.sampleRateHz === labOutputDevice.value?.sampleRateHz &&
        device.channels === labOutputDevice.value?.channels &&
        device.sampleRateHz !== null
          ? [index]
          : [],
      )
      if (matches.length === 1) {
        labOutput.value = String(matches[0])
      } else {
        labOutputDevice.value = null
        labOutput.value = 'unavailable'
      }
    } else if (!loaded.value && labOutput.value !== '' && labOutput.value !== 'unavailable') {
      labOutput.value = 'unavailable'
    } else if (selectedIndex !== null) {
      const restored = snapshot.selectedIndex
      if (restored != null && snapshot.devices[restored]?.sampleRateHz != null) {
        labOutput.value = String(restored)
      } else {
        labOutputDevice.value = null
        labOutput.value = 'unavailable'
      }
    }
    loaded.value = true
  } catch {
    devices.value = []
    loaded.value = false
    if (labOutput.value !== '') {
      labOutputDevice.value = null
      labOutput.value = 'unavailable'
    }
    error.value = 'Could not inspect output devices. Try refreshing.'
  } finally {
    busy.value = false
  }
}

async function play(index: number) {
  if (busy.value || playing.value !== null || labBusy.value || lab.value.running) return
  playing.value = index
  error.value = null
  message.value = null
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    await window.__TAURI__.core.invoke('play_test_tone', { generation: generation.value, index })
    message.value = 'Test tone finished. Did you hear it?'
  } catch (cause) {
    error.value = typeof cause === 'string' && playbackErrors.has(cause)
      ? cause
      : 'Could not play the test tone. Check the device and try again.'
  } finally {
    playing.value = null
  }
}

onMounted(pollLab)
onBeforeUnmount(() => { disposed = true; labRequest++; suggestRequest++; clearTimeout(timer); clearControl() })
</script>

<template>
  <main class="probe-shell">
    <section class="panel utility-panel" aria-labelledby="probe-title">
      <div class="panel-heading">
        <div>
          <p class="eyebrow">Desktop experiment</p>
          <h1 id="probe-title">Output devices</h1>
          <p class="preview-note">Inspect outputs and play a short test tone.</p>
        </div>
      </div>
      <button type="button" class="preview-action" :disabled="busy || playing !== null || lab.running || labBusy" @click="refresh">
        {{ busy ? 'Inspecting…' : 'Refresh devices' }}
      </button>
      <p v-if="error" class="command-error" role="alert">{{ error }}</p>
      <p v-if="loaded && devices.length === 0" role="status">No output devices found.</p>
      <ul v-if="devices.length" class="probe-list" aria-label="Available output devices">
        <li v-for="(device, index) in devices" :key="index" class="selected-device">
          <div>
            <strong>{{ device.label }}{{ device.isDefault ? ' (default)' : '' }}</strong>
            <span v-if="device.sampleRateHz !== null && device.channels !== null">{{ device.sampleRateHz.toLocaleString() }} Hz · {{ device.channels }} channels</span>
            <span v-else>Default format unavailable</span>
          </div>
          <button type="button" class="preview-action probe-play" :disabled="busy || playing !== null || lab.running || labBusy || device.sampleRateHz === null" :aria-label="`Play test tone on ${device.label}`" @click="play(index)">
            {{ playing === index ? 'Playing…' : 'Test sound' }}
          </button>
        </li>
      </ul>
      <p v-if="message" role="status">{{ message }}</p>
    </section>
    <section class="panel utility-panel" aria-labelledby="lab-title">
      <p class="eyebrow">Controlled network experiment</p>
      <h2 id="lab-title">Paired audio lab</h2>
      <p class="preview-note">Uses a public test invitation. Use only on a controlled network; this is not product pairing.</p>
      <p v-if="restoredLab" class="preview-note">Listener settings below apply to the next start; the running listener may use different settings.</p>
      <p v-if="lab.running && lab.mediaPort != null" class="preview-note">Running listener UDP port: {{ lab.mediaPort }}</p>
      <label for="lab-output">Playback output</label>
      <select id="lab-output" v-model="labOutput" :disabled="lab.running || labBusy || busy">
        <option value="">System default output</option>
        <option v-if="labOutput === 'unavailable'" value="unavailable" disabled>Previously selected output unavailable</option>
        <option v-for="(device, index) in devices" :key="index" :value="String(index)" :disabled="device.sampleRateHz === null">{{ device.label }}{{ device.isDefault ? ' (default)' : '' }}</option>
      </select>
      <p v-if="labOutput === 'unavailable'" role="status">Select an available output before starting the listener.</p>
      <label for="lab-host">Desktop address for Android control</label>
      <input id="lab-host" v-model="labHost" :disabled="lab.running || labBusy" autocomplete="off" />
      <p class="preview-note">For ADB reverse use 127.0.0.1 here; enter the UDP address separately in Android (typically 10.0.2.2 for an emulator).</p>
      <label for="android-host">Android device IPv4 (optional)</label>
      <input id="android-host" v-model="androidHost" :disabled="lab.running || labBusy || suggestBusy" autocomplete="off" @input="invalidateHostSuggestion" />
      <button type="button" class="preview-action" :disabled="lab.running || labBusy || suggestBusy" @click="suggestHost">{{ suggestBusy ? 'Checking route…' : 'Suggest desktop address' }}</button>
      <p v-if="suggestError" class="command-error" role="alert">{{ suggestError }}</p>
      <p v-if="suggestedHost" role="status">Suggested desktop address: {{ suggestedHost }} <button type="button" class="preview-action" :disabled="lab.running || labBusy" @click="labHost = suggestedHost">Use address</button></p>
      <p class="preview-note">The suggestion follows the current route to Android; it does not verify connectivity.</p>
      <label for="lab-media-port">Receiver UDP port</label>
      <input id="lab-media-port" v-model="mediaPort" inputmode="numeric" maxlength="5" :disabled="lab.running || labBusy" :aria-invalid="!mediaPortValid" aria-describedby="lab-media-port-help" />
      <p id="lab-media-port-help" class="preview-note">{{ lab.running ? "For direct capture, use the running listener UDP port on Android. With a relay, use its listen port on Android." : "For direct capture, set Android's Desktop UDP port to this value. With a relay, use its forwarding port here and its listen port on Android." }}</p>
      <p v-if="!mediaPortValid" class="command-error" role="alert">Enter a receiver UDP port from 1 to 65535.</p>
      <label for="lab-buffer">Startup buffer</label>
      <select id="lab-buffer" v-model.number="startupBufferMs" :disabled="lab.running || labBusy">
        <option v-for="value in startupBufferChoices" :key="value" :value="value">{{ value }} ms</option>
      </select>
      <p class="preview-note">Initial playout wait; restart the listener to change it. This is not an end-to-end latency measurement.</p>
      <label for="lab-playout">Playout mode</label>
      <select id="lab-playout" v-model="adaptive" :disabled="lab.running || labBusy">
        <option :value="false">Fixed</option>
        <option :value="true">Adaptive (experimental)</option>
      </select>
      <p class="preview-note">Adaptive mode may pause audio while buffering; restart the listener to change modes.</p>
      <button type="button" class="preview-action" :disabled="lab.running || labBusy || busy" @click="clearLabSettings">Reset lab settings</button>
      <button type="button" class="preview-action" :disabled="labBusy || suggestBusy || (!lab.running && (playing !== null || busy || labOutput === 'unavailable' || !mediaPortValid))" @click="toggleLab">{{ lab.running ? 'Stop listener' : 'Start listener' }}</button>
      <p v-if="playing !== null" class="preview-note">Wait for the test sound to finish before starting the listener.</p>
      <p v-else-if="lab.running" class="preview-note">Stop the listener before testing an output.</p>
      <p role="status">{{ lab.status }}</p>
      <p v-if="labError" class="command-error" role="alert">{{ labError }}</p>
      <div v-if="lab.running && lab.volume != null && lab.muted != null">
        <label for="lab-volume">Receiver volume: {{ lab.volume }}%</label>
        <input id="lab-volume" type="range" min="0" max="100" :value="lab.volume" :disabled="controlBusy || labBusy" @change="changeLabVolume" />
        <button type="button" class="preview-action" :disabled="controlBusy || labBusy" :aria-pressed="lab.muted" @click="controlLab(null, !lab.muted)">{{ lab.muted ? 'Unmute' : 'Mute' }}</button>
        <p v-if="controlBusy" role="status">Waiting for the listener to confirm the adjustment…</p>
        <p class="preview-note">The displayed values are confirmed by the listener; system volume is unchanged.</p>
        <p v-if="controlError" class="command-error" role="alert">{{ controlError }}</p>
      </div>
      <section v-if="lab.sessionActive" aria-labelledby="lab-signal-title">
        <h3 id="lab-signal-title">Last reported decoded interval</h3>
        <p v-if="!lab.signal">Waiting for a signal observation.</p>
        <p v-else-if="lab.signal.decoded === 0">No decoded frames in this interval.</p>
        <p v-else>{{ lab.signal.decoded }} decoded frames; peak {{ lab.signal.peak?.toFixed(6) }}.</p>
        <p v-if="lab.signal && lab.signalStale" role="status">Stale observation: no update for at least 3 seconds.</p>
        <p class="preview-note">Approximately one second, before output gain and excluding concealment. This does not confirm audible playback.</p>
      </section>
      <section v-if="lab.metrics || lab.events?.length || (lab.sessionActive && lab.signal)" aria-labelledby="lab-evidence-title">
        <h3 id="lab-evidence-title">{{ lab.metrics ? (lab.sessionActive ? 'Session observations' : 'Last session observations') : 'Session evidence' }}</h3>
        <dl v-if="lab.metrics" class="lab-metrics">
          <div><dt>Accepted packets</dt><dd>{{ lab.metrics.packets }}</dd></div>
          <div v-if="lab.metrics.media"><dt>Media arrival</dt><dd>{{ !lab.sessionActive ? 'Last reported: ' : '' }}{{ lab.metrics.media === 'waiting' ? 'Waiting for first packet' : lab.metrics.media === 'receiving' ? 'Receiving packets' : 'No recent packets' }}</dd></div>
          <div><dt>Packets rejected</dt><dd>{{ lab.metrics.rejected }}</dd></div>
          <div v-if="lab.metrics.late != null"><dt>Late packets</dt><dd>{{ lab.metrics.late }}</dd></div>
          <div v-if="lab.metrics.resyncDropped != null"><dt>Frames dropped on resync</dt><dd>{{ lab.metrics.resyncDropped }}</dd></div>
          <div><dt>Decoded frames</dt><dd>{{ lab.metrics.decoded }}</dd></div>
          <div><dt>Decoded peak</dt><dd>{{ lab.metrics.peak }}</dd></div>
          <div><dt>Concealed frames</dt><dd>{{ lab.metrics.plc }}</dd></div>
          <div><dt>Decode errors</dt><dd>{{ lab.metrics.decodeErrors }}</dd></div>
          <div><dt>Buffered frames</dt><dd>{{ lab.metrics.buffered }}</dd></div>
          <div v-if="lab.metrics.playout"><dt>Playout mode</dt><dd>{{ lab.metrics.playout === 'adaptive' ? 'Adaptive (experimental)' : 'Fixed' }}</dd></div>
          <template v-if="lab.metrics.adaptive">
            <div><dt>Adaptive target</dt><dd>{{ lab.metrics.adaptive.target ? `${lab.metrics.adaptive.target} frames (${lab.metrics.adaptive.min}–${lab.metrics.adaptive.max})` : 'Awaiting packet for this epoch' }}</dd></div>
            <div><dt>Smoothed arrival jitter</dt><dd>{{ lab.metrics.adaptive.jitterMs }} ms</dd></div>
            <div><dt>Gated ticks</dt><dd>{{ lab.metrics.adaptive.gated }}</dd></div>
            <div><dt>Forced playouts</dt><dd>{{ lab.metrics.adaptive.forced }}</dd></div>
            <div><dt>Stream resets</dt><dd>{{ lab.metrics.adaptive.resets }}</dd></div>
          </template>
          <div><dt>Output</dt><dd>{{ !lab.sessionActive && lab.metrics.output !== 'stopped' ? 'Last reported: ' : '' }}{{ lab.metrics.output === 'recovering' ? 'Recovering' : lab.metrics.output === 'active' ? 'Active' : lab.metrics.output === 'stopped' ? 'Stopped' : 'Software check' }}</dd></div>
        </dl>
        <button type="button" class="preview-action" @click="copyLabEvidence">Copy session evidence</button>
        <button type="button" class="preview-action" @click="shareLabEvidence">Share session evidence</button>
        <p v-if="labEvidenceMessage" role="status">{{ labEvidenceMessage }}</p>
        <p v-if="lab.metrics" class="preview-note">{{ lab.sessionActive ? 'Updated at session start, on media or output state changes, and every five seconds.' : lab.metrics.output === 'stopped' ? 'Final update after the stop drain.' : 'Last available update; the session may have ended before its final report.' }} Packet and decode counts and peak are cumulative; they do not confirm audible playback.</p>
        <p v-if="lab.metrics?.media" class="preview-note">Media arrival reports accepted packets within the past two seconds; a gap does not identify its cause.</p>
        <p v-if="lab.metrics?.late != null" class="preview-note">Late packets missed their playout deadline; resync drops are buffered frames discarded when the jitter buffer resynchronizes.</p>
        <p v-if="lab.metrics?.adaptive" class="preview-note">Adaptive target, jitter, gated ticks and forced playouts reset with the stream epoch; stream resets count the session's resets.</p>
      </section>
      <section v-if="lab.events?.length" aria-labelledby="lab-events-title">
        <h3 id="lab-events-title">Recent listener events</h3>
        <ol class="lab-events">
          <li v-for="(event, index) in lab.events" :key="`${index}-${event}`"><code>{{ event }}</code></li>
        </ol>
        <p class="preview-note">The probe retains the latest 32 lifecycle transitions in arrival order.</p>
      </section>
      <template v-if="lab.invitation">
        <label v-if="lab.running && lab.mediaPort != null"><input id="lab-direct-port" v-model="includeDirectPort" type="checkbox" /> Include UDP port for direct connection</label>
        <p class="preview-note">For a relay, leave the port out and configure the Android UDP destination separately.</p>
        <img v-if="invitationQr" class="lab-invitation-qr" :src="invitationQr" alt="QR code for the public lab invitation" />
        <p v-if="invitationQrError" class="command-error" role="alert">Could not display the QR code. Copy the invitation below instead.</p>
        <label for="lab-invitation">Lab invitation payload</label>
        <textarea id="lab-invitation" :value="displayedInvitation" readonly rows="4" @focus="($event.target as HTMLTextAreaElement).select()" />
        <button type="button" class="preview-action" @click="copyInvitation">Copy invitation</button>
        <button type="button" class="preview-action" @click="shareInvitation">Share invitation</button>
        <p v-if="invitationCopyMessage" role="status">{{ invitationCopyMessage }}</p>
        <p class="preview-note">Scan with the Android audio lab or copy this payload. Check its UDP address before capture.</p>
      </template>
    </section>
  </main>
</template>
