<script setup lang="ts">
import { computed } from 'vue'
import { Activity, Check, CircleAlert, Headphones, Volume2, VolumeX } from '@lucide/vue'

import type { OutputDevice, OutputHealth, OutputSnapshot } from '../types'

const props = defineProps<{
  devices: OutputDevice[]
  selectedId: string | null
  output: OutputSnapshot
  gain: number
  muted: boolean
  busy: boolean
  preview?: boolean
}>()

const emit = defineEmits<{
  select: [id: string]
  gain: [gain: number]
  mute: [muted: boolean]
}>()

const selectedDevice = computed(() => props.devices.find((device) => device.id === props.selectedId) ?? null)
const volumeLabel = computed(() => (props.muted ? 'Muted' : `${Math.round(props.gain * 100)}%`))

const healthLabels: Record<OutputHealth, string> = {
  healthy: 'Healthy',
  callbackCapacityExceeded: 'Callback capacity exceeded',
  streamError: 'Stream error',
}

const healthDetails: Record<OutputHealth, string> = {
  healthy: 'The output callback is running inside its bounded path.',
  callbackCapacityExceeded: 'A callback exceeded the preallocated safe output bound.',
  streamError: 'The backend reported an asynchronous stream fault.',
}
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

    <div class="output-status" :class="`health-${output.health}`" aria-label="Output health">
      <span class="output-status-icon" aria-hidden="true">
        <CircleAlert v-if="output.health === 'streamError'" :size="16" />
        <Activity v-else-if="output.health === 'callbackCapacityExceeded'" :size="16" />
        <Check v-else :size="16" />
      </span>
      <div>
        <p class="output-status-line">{{ output.streamActive ? 'Stream active' : 'Stream idle' }} · {{ healthLabels[output.health] }}</p>
        <p class="output-status-detail">{{ healthDetails[output.health] }}</p>
        <p v-if="output.faultReported" class="output-status-fault">A stream fault was reported; reselect or reconnect the output.</p>
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

    <div class="volume-block">
      <div class="volume-heading">
        <span class="volume-icon" :class="{ muted }" aria-hidden="true">
          <VolumeX v-if="muted" :size="16" />
          <Volume2 v-else :size="16" />
        </span>
        <label class="field-label volume-label" for="output-volume">{{ preview ? 'Simulated volume' : 'Volume' }}</label>
        <span class="volume-value">{{ volumeLabel }}</span>
      </div>
      <input
        id="output-volume"
        class="volume-slider"
        type="range"
        min="0"
        max="1"
        step="0.01"
        :value="gain"
        :disabled="busy"
        aria-label="Volume level"
        @input="emit('gain', Number(($event.target as HTMLInputElement).value))"
      />
      <button
        class="mute-toggle"
        :class="{ muted }"
        type="button"
        :disabled="busy"
        :aria-pressed="muted"
        @click="emit('mute', !muted)"
      >
        <VolumeX v-if="muted" :size="14" aria-hidden="true" />
        <Volume2 v-else :size="14" aria-hidden="true" />
        {{ muted ? 'Unmute' : 'Mute' }}
      </button>
    </div>
  </section>
</template>
