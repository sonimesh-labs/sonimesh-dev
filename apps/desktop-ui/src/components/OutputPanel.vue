<script setup lang="ts">
import { computed } from 'vue'
import { Check, Headphones } from '@lucide/vue'

import type { OutputDevice } from '../types'

const props = defineProps<{
  devices: OutputDevice[]
  selectedId: string | null
  busy: boolean
}>()

const emit = defineEmits<{
  select: [id: string]
}>()

const selectedDevice = computed(() => props.devices.find((device) => device.id === props.selectedId) ?? null)
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
    <p v-if="selectedDevice" class="hint device-summary">
      <Check :size="15" aria-hidden="true" />
      {{ selectedDevice.sampleRateHz.toLocaleString() }} Hz · {{ selectedDevice.channels }} channels
    </p>
  </section>
</template>
