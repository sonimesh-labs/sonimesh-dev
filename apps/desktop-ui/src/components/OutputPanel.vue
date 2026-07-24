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
  <section class="panel utility-panel" aria-labelledby="output-heading">
    <div class="panel-heading">
      <div class="panel-icon"><Headphones :size="19" aria-hidden="true" /></div>
      <div>
        <p class="eyebrow">Listening output</p>
        <h2 id="output-heading">Where sound lands</h2>
      </div>
    </div>

    <div v-if="selectedDevice" class="selected-device">
      <span class="device-check"><Check :size="16" aria-hidden="true" /></span>
      <div>
        <strong>{{ selectedDevice.label }}</strong>
        <span>{{ selectedDevice.sampleRateHz.toLocaleString() }} Hz · {{ selectedDevice.channels }} channels</span>
      </div>
    </div>

    <label class="field-label" for="output-device">Change output device</label>
    <div class="select-wrap">
      <select id="output-device" class="device-select" :value="selectedId ?? ''" :disabled="busy" @change="emit('select', ($event.target as HTMLSelectElement).value)">
        <option v-for="device in devices" :key="device.id" :value="device.id">
          {{ device.label }}{{ device.isDefault ? ' (default)' : '' }}
        </option>
      </select>
    </div>
  </section>
</template>
