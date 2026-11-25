<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'

type LoopbackSnapshot = { running: boolean; endpointName: string; sampleRate: number; channels: number; samples: number; peak: number; error: string | null }

const names = ref<string[]>([])
const generation = ref(0)
const selected = ref('')
const loaded = ref(false)
const busy = ref(false)
const phase = ref<'idle' | 'waiting' | 'open'>('idle')
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const loopback = ref<LoopbackSnapshot | null>(null)
const loopbackBusy = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined
let loopbackTimer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let statusRequest = 0

function sourceName() {
  return names.value[Number(selected.value)] ?? 'selected source'
}

async function refresh() {
  busy.value = true
  error.value = null
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
  try {
    loopback.value = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_status')
  } catch (cause) {
    if (loopback.value) loopback.value = { ...loopback.value, error: String(cause) }
  }
  if (!disposed && loopback.value?.running) loopbackTimer = setTimeout(pollLoopback, 1000)
}

async function toggleLoopback() {
  if (!window.__TAURI__) return
  loopbackBusy.value = true
  try {
    const result = await window.__TAURI__.core.invoke<LoopbackSnapshot>(loopback.value?.running ? 'loopback_stop' : 'loopback_start')
    loopback.value = result
    if (result.running) void pollLoopback()
    else clearTimeout(loopbackTimer)
  } catch (cause) {
    loopback.value = { running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: String(cause) }
  } finally {
    loopbackBusy.value = false
  }
}

async function toggle(mode: 'connect' | 'wait' = 'connect') {
  if (!window.__TAURI__) return
  busy.value = true
  error.value = null
  message.value = null
  statusRequest++
  clearTimeout(timer)
  try {
    if (phase.value !== 'idle') {
      await window.__TAURI__.core.invoke('bluetooth_disconnect')
      phase.value = 'idle'
      clearTimeout(timer)
      message.value = 'Disconnected.'
    } else {
      await window.__TAURI__.core.invoke(mode === 'wait' ? 'bluetooth_prepare' : 'bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
      phase.value = mode === 'wait' ? 'waiting' : 'open'
      message.value = mode === 'wait'
        ? `Waiting for the phone to connect to ${sourceName()} through Bluetooth.`
        : `System playback connection opened for ${sourceName()}.`
      void poll()
    }
  } catch (cause) {
    error.value = String(cause)
    if (phase.value !== 'idle') void poll()
  } finally {
    busy.value = false
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
    if (loopbackState.running) void pollLoopback()
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  } finally {
    if (!disposed && request === statusRequest) busy.value = false
  }
})

onBeforeUnmount(() => { disposed = true; statusRequest++; clearTimeout(timer); clearTimeout(loopbackTimer) })
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
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || selected === ''" @click="toggle('wait')">Wait for phone to connect</button>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" class="command-error" role="alert">{{ error }}</p>
    <h3>System output PCM check</h3>
    <p class="preview-note">Measures the default Windows playback mix; other apps' audio may also appear here.</p>
    <button type="button" class="preview-action" :disabled="loopbackBusy" @click="toggleLoopback">{{ loopbackBusy ? 'Starting...' : loopback?.running ? 'Stop PCM check' : 'Start PCM check' }}</button>
    <p v-if="loopback?.running && loopback.samples === 0" role="status">Waiting for Windows audio samples.</p>
    <p v-if="loopback && (loopback.running || loopback.sampleRate > 0)" role="status">{{ loopback.endpointName }} · {{ loopback.sampleRate }} Hz, {{ loopback.channels }} channels; {{ loopback.samples }} samples; recent peak {{ (loopback.peak * 100).toFixed(1) }}%.</p>
    <p v-if="loopback?.error" class="command-error" role="alert">{{ loopback.error }}</p>
  </section>
</template>
