<script setup lang="ts">
import { Headphones } from '@lucide/vue'

import type { OutputDevice } from '../types'

defineProps<{
  devices: OutputDevice[]
  selectedId: string | null
  busy: boolean
}>()

const emit = defineEmits<{
  select: [id: string]
}>()
</script>

<template>
  <section class="panel" aria-labelledby="output-heading">
    <div class="panel-heading compact-heading">
      <div>
        <p class="eyebrow">Output</p>
        <h2 id="output-heading">Listening device</h2>
      </div>
      <Headphones :size="21" aria-hidden="true" />
    </div>
    <label class="field-label" for="output-device">Device</label>
    <select id="output-device" class="device-select" :value="selectedId ?? ''" :disabled="busy" @change="emit('select', ($event.target as HTMLSelectElement).value)">
      <option v-for="device in devices" :key="device.id" :value="device.id">
        {{ device.label }}{{ device.isDefault ? ' (default)' : '' }}
      </option>
    </select>
    <p v-if="devices.find((device) => device.id === selectedId)" class="hint">
      {{ devices.find((device) => device.id === selectedId)?.sampleRateHz.toLocaleString() }} Hz · {{ devices.find((device) => device.id === selectedId)?.channels }} channels
    </p>
  </section>
</template>
