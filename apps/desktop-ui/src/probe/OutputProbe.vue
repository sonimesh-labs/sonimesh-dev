<script setup lang="ts">
import { onMounted, ref } from 'vue'

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
  if (busy.value) return
  busy.value = true
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
</script>

<template>
  <main class="probe-shell">
    <section class="panel utility-panel" aria-labelledby="probe-title">
      <div class="panel-heading">
        <div>
          <p class="eyebrow">Desktop experiment</p>
          <h1 id="probe-title">Output devices</h1>
          <p class="preview-note">Inspect outputs and play a short test tone. Receiver sessions are not connected yet.</p>
        </div>
      </div>
      <button type="button" class="preview-action" :disabled="busy || playing !== null" @click="refresh">
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
  </main>
</template>
