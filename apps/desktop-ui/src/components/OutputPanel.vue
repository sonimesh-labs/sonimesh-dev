<script setup lang="ts">
import { computed } from 'vue'
import { Check, Headphones } from '@lucide/vue'

import type { OutputDevice } from '../types'

const props = defineProps<{
  devices: OutputDevice[]
  selectedId: string | null
  busy: boolean
  preview?: boolean
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
        <p class="eyebrow">{{ preview ? 'Output preview' : 'Listening output' }}</p>
        <h2 id="output-heading">{{ preview ? 'Simulated output' : 'Where sound lands' }}</h2>
        <p v-if="preview" class="preview-note">No hardware is inspected or changed.</p>
      </div>
    </div>

    <div v-if="selectedDevice" class="selected-device">
      <span class="device-check"><Check :size="16" aria-hidden="true" /></span>
      <div>
        <strong>{{ selectedDevice.label }}</strong>
        <span>{{ preview ? 'Synthetic preview · ' : '' }}{{ selectedDevice.sampleRateHz.toLocaleString() }} Hz · {{ selectedDevice.channels }} channels</span>
      </div>
    </div>

    <label class="field-label" for="output-device">{{ preview ? 'Change simulated output' : 'Change output device' }}</label>
    <div class="select-wrap">
      <select id="output-device" class="device-select" :value="selectedId ?? ''" :disabled="busy" @change="emit('select', ($event.target as HTMLSelectElement).value)">
        <option v-for="device in devices" :key="device.id" :value="device.id">
          {{ device.label }}{{ device.isDefault ? ' (default)' : '' }}
        </option>
      </select>
    </div>
  </section>
</template>
