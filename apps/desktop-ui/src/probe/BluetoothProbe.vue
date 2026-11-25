<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'

type LoopbackSnapshot = { running: boolean; endpointName: string; sampleRate: number; channels: number; samples: number; peak: number; error: string | null }
type LoopbackEndpoint = { id: string; name: string; isDefault: boolean }

const names = ref<string[]>([])
const LOOPBACK_NO_SAMPLES_DELAY_MS = 5000
const generation = ref(0)
const selected = ref('')
const loaded = ref(false)
const busy = ref(false)
const phase = ref<'idle' | 'waiting' | 'open'>('idle')
const error = ref<string | null>(null)
const suggestPhoneInitiated = ref(false)
const message = ref<string | null>(null)
const loopback = ref<LoopbackSnapshot | null>(null)
const loopbackEndpoints = ref<LoopbackEndpoint[]>([])
const loopbackEndpointId = ref('')
const loopbackBusy = ref(false)
const loopbackError = ref<string | null>(null)
const loopbackNoSamplesLong = ref(false)
const combinedMonitorOwned = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined
let loopbackTimer: ReturnType<typeof setTimeout> | undefined
let loopbackStartedAt: number | undefined
let disposed = false
let statusRequest = 0
let loopbackRequest = 0

function sourceName() {
  return names.value[Number(selected.value)] ?? 'selected source'
}

function updateLoopbackWaitState(snapshot: LoopbackSnapshot) {
  if (!snapshot.running || snapshot.samples > 0) {
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
    return
  }
  loopbackStartedAt ??= Date.now()
  loopbackNoSamplesLong.value = Date.now() - loopbackStartedAt >= LOOPBACK_NO_SAMPLES_DELAY_MS
}

async function refresh() {
  busy.value = true
  error.value = null
  suggestPhoneInitiated.value = false
  selected.value = ''
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    const result = await window.__TAURI__.core.invoke<{ generation: number; names: string[] }>('bluetooth_candidates')
    names.value = result.names
    generation.value = result.generation
    loaded.value = true
  } catch (cause) {
    error.value = String(cause)
  } finally {
    busy.value = false
  }
}

async function poll() {
  if (disposed || phase.value === 'idle') return
  const request = statusRequest
  try {
    const state = await window.__TAURI__!.core.invoke<'idle' | 'waiting' | 'open'>('bluetooth_status')
    if (disposed || request !== statusRequest) return
    if (state === 'idle') {
      phase.value = 'idle'
      message.value = 'Windows closed the playback connection.'
      await stopCombinedMonitor()
      return
    }
    if (state === 'open' && phase.value === 'waiting') message.value = `System playback connection opened for ${sourceName()}.`
    phase.value = state
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  }
  if (!disposed && request === statusRequest && phase.value !== 'idle') timer = setTimeout(poll, 1000)
}

async function pollLoopback() {
  if (disposed || !loopback.value?.running) return
  const request = loopbackRequest
  try {
    const snapshot = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_status')
    if (disposed || request !== loopbackRequest) return
    loopback.value = snapshot
    updateLoopbackWaitState(snapshot)
  } catch (cause) {
    if (!disposed && request === loopbackRequest && loopback.value) {
      loopback.value = { ...loopback.value, error: String(cause) }
    }
  }
  if (!disposed && request === loopbackRequest && loopback.value?.running) loopbackTimer = setTimeout(pollLoopback, 1000)
}

async function refreshLoopbackEndpoints() {
  if (!window.__TAURI__) return
  loopbackBusy.value = true
  loopbackError.value = null
  try {
    loopbackEndpoints.value = await window.__TAURI__.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? []
    if (!loopbackEndpoints.value.some(endpoint => endpoint.id === loopbackEndpointId.value)) {
      loopbackEndpointId.value = loopbackEndpoints.value.find(endpoint => endpoint.isDefault)?.id ?? loopbackEndpoints.value[0]?.id ?? ''
    }
  } catch (cause) {
    loopbackError.value = String(cause)
  } finally {
    loopbackBusy.value = false
  }
}

async function toggleLoopback() {
  if (!window.__TAURI__ || (!loopback.value?.running && !loopbackEndpointId.value)) return
  loopbackBusy.value = true
  loopbackError.value = null
  if (loopback.value?.running) {
    loopbackRequest++
    clearTimeout(loopbackTimer)
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
  }
  try {
    const result = loopback.value?.running
      ? await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_stop')
      : await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: loopbackEndpointId.value })
    loopback.value = result
    if (result.running) {
      loopbackStartedAt = Date.now()
      loopbackNoSamplesLong.value = false
      void pollLoopback()
    } else {
      loopbackRequest++
      clearTimeout(loopbackTimer)
      loopbackStartedAt = undefined
      loopbackNoSamplesLong.value = false
      combinedMonitorOwned.value = false
    }
  } catch (cause) {
    loopbackError.value = String(cause)
  } finally {
    loopbackBusy.value = false
  }
}

async function stopCombinedMonitor() {
  if (!combinedMonitorOwned.value || !window.__TAURI__) return
  loopbackRequest++
  clearTimeout(loopbackTimer)
  loopbackBusy.value = true
  try {
    loopback.value = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_stop')
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
    combinedMonitorOwned.value = false
  } catch (cause) {
    loopbackError.value = String(cause)
    if (loopback.value?.running) void pollLoopback()
  } finally {
    loopbackBusy.value = false
  }
}

async function toggle(mode: 'connect' | 'wait' = 'connect', monitorOutput = false, fallbackToWait = false) {
  if (!window.__TAURI__) return
  busy.value = true
  if (monitorOutput) loopbackBusy.value = true
  error.value = null
  suggestPhoneInitiated.value = false
  message.value = null
  statusRequest++
  clearTimeout(timer)
  let loopbackStartedHere = false
  let bluetoothAttempted = false
  let directError: unknown
  try {
    if (phase.value !== 'idle') {
      await window.__TAURI__.core.invoke('bluetooth_disconnect')
      phase.value = 'idle'
      clearTimeout(timer)
      await stopCombinedMonitor()
      message.value = 'Disconnected.'
    } else {
      if (monitorOutput && !loopback.value?.running) {
        loopbackError.value = null
        try {
          loopback.value = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: loopbackEndpointId.value })
          if (!loopback.value.running) throw new Error('Windows output monitoring did not start.')
          loopbackStartedHere = true
          combinedMonitorOwned.value = true
          loopbackStartedAt = Date.now()
          loopbackNoSamplesLong.value = false
        } catch (cause) {
          loopbackError.value = String(cause)
          throw cause
        }
      }
      bluetoothAttempted = true
      let connectionMode = mode
      if (fallbackToWait && mode === 'connect') {
        try {
          await window.__TAURI__.core.invoke('bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
        } catch (cause) {
          directError = cause
          await window.__TAURI__.core.invoke('bluetooth_prepare', { generation: generation.value, index: Number(selected.value) })
          connectionMode = 'wait'
          error.value = String(cause)
        }
      } else {
        await window.__TAURI__.core.invoke(mode === 'wait' ? 'bluetooth_prepare' : 'bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
      }
      phase.value = connectionMode === 'wait' ? 'waiting' : 'open'
      message.value = connectionMode === 'wait'
        ? `${directError ? 'Direct connection failed. ' : ''}Waiting for the phone to connect to ${sourceName()} through Bluetooth.${monitorOutput ? ' Windows output monitoring is active.' : ''}`
        : `System playback connection opened for ${sourceName()}.`
      if (loopbackStartedHere) void pollLoopback()
      void poll()
    }
  } catch (cause) {
    if (loopbackStartedHere) {
      await stopCombinedMonitor()
    }
    if (fallbackToWait && directError) {
      error.value = `Direct connection failed: ${String(directError)} Phone-initiated wait also failed: ${String(cause)}`
    } else if (monitorOutput && !bluetoothAttempted) {
      loopbackError.value ||= String(cause)
    } else {
      error.value = String(cause)
    }
    suggestPhoneInitiated.value = !fallbackToWait && mode === 'connect' && phase.value === 'idle'
    if (phase.value !== 'idle') void poll()
  } finally {
    busy.value = false
    if (monitorOutput) loopbackBusy.value = false
  }
}

onMounted(async () => {
  if (!window.__TAURI__) return
  busy.value = true
  const request = statusRequest
  try {
    const state = await window.__TAURI__.core.invoke<'idle' | 'waiting' | 'open'>('bluetooth_status')
    if (!disposed && request === statusRequest && (state === 'open' || state === 'waiting')) {
      phase.value = state
      message.value = state === 'waiting'
        ? 'Waiting for the phone to connect through Bluetooth.'
        : 'System playback connection is open.'
      void poll()
    }
    const loopbackState = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_status')
    loopback.value = loopbackState
    if (loopbackState.running) {
      loopbackStartedAt = Date.now()
      loopbackNoSamplesLong.value = false
      void pollLoopback()
    }
    await refreshLoopbackEndpoints()
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  } finally {
    if (!disposed && request === statusRequest) busy.value = false
  }
})

onBeforeUnmount(() => { disposed = true; statusRequest++; loopbackRequest++; clearTimeout(timer); clearTimeout(loopbackTimer) })
</script>

<template>
  <section class="panel utility-panel" aria-labelledby="bluetooth-title">
    <p class="eyebrow">Windows experiment</p>
    <h2 id="bluetooth-title">Bluetooth system playback</h2>
    <p class="preview-note">Connect a paired source through Windows. Audio is played by the system and does not enter the SoniMesh mixer.</p>
    <button type="button" class="preview-action" :disabled="busy || phase !== 'idle'" @click="refresh">Refresh Bluetooth sources</button>
    <p v-if="loaded && names.length === 0" role="status">No playback sources found. Pair a compatible device in Windows, then refresh.</p>
    <template v-if="names.length">
      <label for="bluetooth-source">Playback source</label>
      <select id="bluetooth-source" v-model="selected" :disabled="busy || phase !== 'idle'">
        <option value="" disabled>Select a source</option>
        <option v-for="(name, index) in names" :key="index" :value="String(index)">{{ name }}</option>
      </select>
    </template>
    <button v-if="names.length || phase !== 'idle'" type="button" class="preview-action" :disabled="busy || (phase === 'idle' && selected === '')" @click="toggle('connect')">{{ phase === 'idle' ? 'Connect Bluetooth' : 'Disconnect Bluetooth' }}</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || selected === ''" @click="toggle('connect', false, true)">Connect or wait for phone</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || selected === ''" @click="toggle('wait')">Wait for phone to connect</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || loopbackBusy || selected === '' || !loopbackEndpointId" @click="toggle('wait', true)">Wait + monitor system output</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || loopbackBusy || selected === '' || !loopbackEndpointId" @click="toggle('connect', true, true)">Connect + monitor (auto wait fallback)</button>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" class="command-error" role="alert">{{ error }}</p>
    <p v-if="suggestPhoneInitiated" role="status">Try waiting for the phone to connect, then start playback from the phone.</p>
    <h3>System output PCM check</h3>
    <p class="preview-note">Measures the selected Windows playback mix; other apps' audio may also appear here.</p>
    <button type="button" class="preview-action" :disabled="loopbackBusy || loopback?.running" @click="refreshLoopbackEndpoints">Refresh output devices</button>
    <label for="loopback-endpoint">Output endpoint</label>
    <select id="loopback-endpoint" v-model="loopbackEndpointId" :disabled="loopbackBusy || loopback?.running">
      <option value="" disabled>Select an output endpoint</option>
      <option v-for="endpoint in loopbackEndpoints" :key="endpoint.id" :value="endpoint.id">{{ endpoint.name }}{{ endpoint.isDefault ? ' (default)' : '' }}</option>
    </select>
    <button type="button" class="preview-action" :disabled="loopbackBusy || (!loopback?.running && !loopbackEndpointId)" @click="toggleLoopback">{{ loopbackBusy ? 'Working...' : loopback?.running ? 'Stop PCM check' : 'Start PCM check' }}</button>
    <p v-if="loopback?.running && loopback.samples === 0" role="status">{{ loopbackNoSamplesLong ? 'No Windows audio samples have arrived yet. Start playback and check the selected output endpoint.' : 'Waiting for Windows audio samples.' }}</p>
    <p v-if="loopback && (loopback.running || loopback.sampleRate > 0)" role="status">{{ loopback.endpointName }} · {{ loopback.sampleRate }} Hz, {{ loopback.channels }} channels; {{ loopback.samples }} samples; recent peak {{ (loopback.peak * 100).toFixed(1) }}%<template v-if="loopback.samples > 0">; {{ loopback.peak > 0.001 ? 'PCM activity detected recently' : 'No recent PCM activity' }}</template>.</p>
    <p v-if="loopbackError || loopback?.error" class="command-error" role="alert">{{ loopbackError || loopback?.error }}</p>
  </section>
</template>
