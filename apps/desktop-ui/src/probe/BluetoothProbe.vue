<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'

const names = ref<string[]>([])
const generation = ref(0)
const selected = ref('')
const loaded = ref(false)
const busy = ref(false)
const connected = ref(false)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
let timer: ReturnType<typeof setTimeout> | undefined
let disposed = false
let statusRequest = 0

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
  if (disposed || !connected.value) return
  const request = statusRequest
  try {
    const open = await window.__TAURI__!.core.invoke<boolean>('bluetooth_status')
    if (disposed || request !== statusRequest) return
    if (!open) {
      connected.value = false
      message.value = 'Windows closed the playback connection.'
      return
    }
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  }
  if (!disposed && request === statusRequest && connected.value) timer = setTimeout(poll, 1000)
}

async function toggle() {
  if (!window.__TAURI__) return
  busy.value = true
  error.value = null
  message.value = null
  statusRequest++
  clearTimeout(timer)
  try {
    if (connected.value) {
      await window.__TAURI__.core.invoke('bluetooth_disconnect')
      connected.value = false
      clearTimeout(timer)
      message.value = 'Disconnected.'
    } else {
      await window.__TAURI__.core.invoke('bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
      connected.value = true
      message.value = 'System playback connection opened.'
      void poll()
    }
  } catch (cause) {
    error.value = String(cause)
    if (connected.value) void poll()
  } finally {
    busy.value = false
  }
}

onMounted(async () => {
  if (!window.__TAURI__) return
  busy.value = true
  const request = statusRequest
  try {
    const open = await window.__TAURI__.core.invoke<boolean>('bluetooth_status')
    if (!disposed && request === statusRequest && open === true) {
      connected.value = true
      message.value = 'System playback connection is open.'
      void poll()
    }
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  } finally {
    if (!disposed && request === statusRequest) busy.value = false
  }
})

onBeforeUnmount(() => { disposed = true; statusRequest++; clearTimeout(timer) })
</script>

<template>
  <section class="panel utility-panel" aria-labelledby="bluetooth-title">
    <p class="eyebrow">Windows experiment</p>
    <h2 id="bluetooth-title">Bluetooth system playback</h2>
    <p class="preview-note">Connect a paired source through Windows. Audio is played by the system and does not enter the SoniMesh mixer.</p>
    <button type="button" class="preview-action" :disabled="busy || connected" @click="refresh">Refresh Bluetooth sources</button>
    <p v-if="loaded && names.length === 0" role="status">No playback sources found. Pair a compatible device in Windows, then refresh.</p>
    <template v-if="names.length">
      <label for="bluetooth-source">Playback source</label>
      <select id="bluetooth-source" v-model="selected" :disabled="busy || connected">
        <option value="" disabled>Select a source</option>
        <option v-for="(name, index) in names" :key="index" :value="String(index)">{{ name }}</option>
      </select>
    </template>
    <button v-if="names.length || connected" type="button" class="preview-action" :disabled="busy || (!connected && selected === '')" @click="toggle">{{ connected ? 'Disconnect Bluetooth' : 'Connect Bluetooth' }}</button>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" class="command-error" role="alert">{{ error }}</p>
  </section>
</template>
