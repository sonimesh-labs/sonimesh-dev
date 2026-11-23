<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'

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
  plc: string; decodeErrors: string; buffered: string; output: 'active' | 'recovering' | 'software-check'
}
interface LabSnapshot { running: boolean; status: string; invitation: string | null; metrics: LabMetrics | null; volume: number | null; muted: boolean | null }
const lab = ref<LabSnapshot>({ running: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
const labHost = ref('127.0.0.1')
const labOutput = ref('')
const startupBufferMs = ref(60)
const labBusy = ref(false)
const labError = ref<string | null>(null)
const controlError = ref<string | null>(null)
const controlBusy = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let labRequest = 0

async function pollLab() {
  const request = labRequest
  try {
    if (!window.__TAURI__) return
    const snapshot = await window.__TAURI__.core.invoke<LabSnapshot>('lab_status')
    if (disposed || request !== labRequest) return
    lab.value = snapshot
    labError.value = null
    if (!disposed && lab.value.running) timer = setTimeout(pollLab, 1000)
  } catch {
    if (!disposed && request === labRequest) {
      labError.value = 'Could not check the listener. Retrying…'
      if (lab.value.running) timer = setTimeout(pollLab, 1000)
    }
  }
}

async function toggleLab() {
  if (labBusy.value || !window.__TAURI__) return
  labBusy.value = true
  labError.value = null
  labRequest++
  clearTimeout(timer)
  try {
    lab.value = lab.value.running
      ? await window.__TAURI__.core.invoke<LabSnapshot>('stop_lab')
      : await window.__TAURI__.core.invoke<LabSnapshot>('start_lab', { host: labHost.value.trim(), generation: generation.value, index: labOutput.value === '' ? null : Number(labOutput.value), startupBufferMs: startupBufferMs.value })
    if (!disposed && lab.value.running) timer = setTimeout(pollLab, 1000)
  } catch (cause) {
    labError.value = typeof cause === 'string' ? cause : 'Could not change listener state.'
    if (cause === 'The listener is already running.') await pollLab()
  } finally {
    labBusy.value = false
  }
}

async function controlLab(volume: number | null, muted: boolean | null) {
  if (!window.__TAURI__ || controlBusy.value || !lab.value.running) return
  controlBusy.value = true
  controlError.value = null
  try {
    await window.__TAURI__.core.invoke('control_lab', { volume, muted })
  } catch (cause) {
    controlError.value = typeof cause === 'string' ? cause : 'Could not change audio control.'
  } finally {
    controlBusy.value = false
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
  'Could not inspect this output.',
  'This output format is not supported.',
  'Could not start this output.',
  'Output stopped or reported a playback error.',
])

async function refresh() {
  if (busy.value || lab.value.running || labBusy.value) return
  busy.value = true
  labOutput.value = ''
  error.value = null
  message.value = null
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    const snapshot = await window.__TAURI__.core.invoke<{ generation: number; devices: OutputDevice[] }>('probe_outputs')
    generation.value = snapshot.generation
    devices.value = snapshot.devices
    loaded.value = true
  } catch {
    devices.value = []
    loaded.value = false
    error.value = 'Could not inspect output devices. Try refreshing.'
  } finally {
    busy.value = false
  }
}

async function play(index: number) {
  if (busy.value || playing.value !== null) return
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

onMounted(refresh)
onBeforeUnmount(() => { disposed = true; labRequest++; clearTimeout(timer) })
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
          <button type="button" class="preview-action probe-play" :disabled="busy || playing !== null || device.sampleRateHz === null" :aria-label="`Play test tone on ${device.label}`" @click="play(index)">
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
      <label for="lab-output">Playback output</label>
      <select id="lab-output" v-model="labOutput" :disabled="lab.running || labBusy || busy">
        <option value="">System default output</option>
        <option v-for="(device, index) in devices" :key="index" :value="String(index)" :disabled="device.sampleRateHz === null">{{ device.label }}{{ device.isDefault ? ' (default)' : '' }}</option>
      </select>
      <label for="lab-host">Desktop address for Android control</label>
      <input id="lab-host" v-model="labHost" :disabled="lab.running || labBusy" autocomplete="off" />
      <p class="preview-note">For ADB reverse use 127.0.0.1 here; enter the UDP address separately in Android (typically 10.0.2.2 for an emulator).</p>
      <label for="lab-buffer">Startup buffer</label>
      <select id="lab-buffer" v-model.number="startupBufferMs" :disabled="lab.running || labBusy">
        <option v-for="value in [20, 40, 60, 80, 100, 120]" :key="value" :value="value">{{ value }} ms</option>
      </select>
      <p class="preview-note">Initial playout wait; restart the listener to change it. This is not an end-to-end latency measurement.</p>
      <button type="button" class="preview-action" :disabled="labBusy" @click="toggleLab">{{ lab.running ? 'Stop listener' : 'Start listener' }}</button>
      <p role="status">{{ lab.status }}</p>
      <p v-if="labError" class="command-error" role="alert">{{ labError }}</p>
      <div v-if="lab.running && lab.volume != null && lab.muted != null">
        <label for="lab-volume">Receiver volume: {{ lab.volume }}%</label>
        <input id="lab-volume" type="range" min="0" max="100" :value="lab.volume" :disabled="controlBusy" @change="changeLabVolume" />
        <button type="button" class="preview-action" :disabled="controlBusy" :aria-pressed="lab.muted" @click="controlLab(null, !lab.muted)">{{ lab.muted ? 'Unmute' : 'Mute' }}</button>
        <p class="preview-note">The displayed values are confirmed by the listener; system volume is unchanged.</p>
        <p v-if="controlError" class="command-error" role="alert">{{ controlError }}</p>
      </div>
      <section v-if="lab.metrics" aria-labelledby="lab-metrics-title">
        <h3 id="lab-metrics-title">Session observations</h3>
        <dl class="lab-metrics">
          <div><dt>Accepted packets</dt><dd>{{ lab.metrics.packets }}</dd></div>
          <div><dt>Packets rejected</dt><dd>{{ lab.metrics.rejected }}</dd></div>
          <div><dt>Decoded frames</dt><dd>{{ lab.metrics.decoded }}</dd></div>
          <div><dt>Decoded peak</dt><dd>{{ lab.metrics.peak }}</dd></div>
          <div><dt>Concealed frames</dt><dd>{{ lab.metrics.plc }}</dd></div>
          <div><dt>Decode errors</dt><dd>{{ lab.metrics.decodeErrors }}</dd></div>
          <div><dt>Buffered frames</dt><dd>{{ lab.metrics.buffered }}</dd></div>
          <div><dt>Output</dt><dd>{{ lab.metrics.output === 'recovering' ? 'Recovering' : lab.metrics.output === 'active' ? 'Active' : 'Software check' }}</dd></div>
        </dl>
        <p class="preview-note">Updated at session start and every five seconds. Counts and peak are cumulative; they do not confirm audible playback.</p>
      </section>
      <template v-if="lab.invitation">
        <label for="lab-invitation">Lab invitation payload</label>
        <textarea id="lab-invitation" :value="lab.invitation" readonly rows="4" @focus="($event.target as HTMLTextAreaElement).select()" />
        <p class="preview-note">Copy this payload into the Android audio lab and configure its UDP address separately.</p>
      </template>
    </section>
  </main>
</template>
