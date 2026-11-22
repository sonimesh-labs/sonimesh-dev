<script setup lang="ts">
import { onMounted, ref } from 'vue'

interface OutputDevice {
  label: string
  sampleRateHz: number
  channels: number
  isDefault: boolean
}

declare global {
  interface Window {
    __TAURI__?: { core: { invoke<T>(command: string): Promise<T> } }
  }
}

const devices = ref<OutputDevice[]>([])
const busy = ref(false)
const error = ref<string | null>(null)
const loaded = ref(false)

async function refresh() {
  if (busy.value) return
  busy.value = true
  error.value = null
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    devices.value = await window.__TAURI__.core.invoke<OutputDevice[]>('probe_outputs')
    loaded.value = true
  } catch {
    error.value = 'Could not inspect output devices. Try refreshing.'
  } finally {
    busy.value = false
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
          <p class="preview-note">Read-only device inspection. Audio sessions and output selection are not connected yet.</p>
        </div>
      </div>
      <button type="button" class="preview-action" :disabled="busy" @click="refresh">
        {{ busy ? 'Inspecting…' : 'Refresh devices' }}
      </button>
      <p v-if="error" class="command-error" role="alert">{{ error }}</p>
      <p v-else-if="loaded && devices.length === 0" role="status">No output devices found.</p>
      <ul v-else-if="devices.length" class="probe-list" aria-label="Available output devices">
        <li v-for="(device, index) in devices" :key="index" class="selected-device">
          <div>
            <strong>{{ device.label }}{{ device.isDefault ? ' (default)' : '' }}</strong>
            <span>{{ device.sampleRateHz.toLocaleString() }} Hz · {{ device.channels }} channels</span>
          </div>
        </li>
      </ul>
    </section>
  </main>
</template>
