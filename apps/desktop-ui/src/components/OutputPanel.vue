<script setup lang="ts">
import { computed } from 'vue'
import { Activity, Check, CircleAlert, Headphones, Volume2, VolumeX } from '@lucide/vue'

import type { OutputDevice, OutputHealth, OutputSnapshot } from '../types'

const props = withDefaults(defineProps<{
  devices: OutputDevice[]
  selectedId: string | null
  output: OutputSnapshot
  gain: number
  muted: boolean
  busy: boolean
  preview?: boolean
  receiverAvailable?: boolean
  streamControlsAvailable?: boolean
}>(), {
  preview: false,
  receiverAvailable: true,
  streamControlsAvailable: true,
})

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

type DisplayStatus = {
  kind: OutputHealth | 'idle'
  line: string
  detail: string
}

const displayStatus = computed<DisplayStatus>(() => {
  if (!props.output.streamActive && !props.output.faultReported) {
    return {
      kind: 'idle',
      line: 'Stream idle · No active output',
      detail: 'No output stream is currently running.',
    }
  }

  // A reported fault must never receive a healthy presentation, even if an
  // adapter produces an inconsistent low-rate snapshot.
  const health = props.output.faultReported && props.output.health === 'healthy'
    ? 'streamError'
    : props.output.health
  const lifecycle = props.output.streamActive ? 'Stream active' : 'Stream idle'

  return {
    kind: health,
    line: `${lifecycle} · ${healthLabels[health]}`,
    detail: healthDetails[health],
  }
})
</script>

<template>
  <section class="panel utility-panel" aria-labelledby="output-heading">
    <div class="panel-heading">
      <div class="panel-icon"><Headphones :size="19" aria-hidden="true" /></div>
      <div>
        <p class="eyebrow">{{ preview ? 'Output preview' : receiverAvailable ? 'Listening output' : 'Windows output' }}</p>
        <h2 id="output-heading">{{ preview ? 'Simulated output' : receiverAvailable ? 'Where sound lands' : 'Preferred output' }}</h2>
        <p v-if="preview" class="preview-note">No hardware is inspected or changed.</p>
        <p v-else-if="!receiverAvailable" class="preview-note">Selected for this app session.</p>
      </div>
    </div>

    <div v-if="selectedDevice" class="selected-device">
      <span class="device-check"><Check :size="16" aria-hidden="true" /></span>
      <div>
        <strong>{{ selectedDevice.label }}</strong>
        <span>{{ preview ? 'Synthetic preview · ' : '' }}{{ selectedDevice.sampleRateHz.toLocaleString() }} Hz · {{ selectedDevice.channels }} channels</span>
      </div>
    </div>

    <div class="output-status" :class="`health-${displayStatus.kind}`" aria-label="Output status">
      <span class="output-status-icon" aria-hidden="true">
        <CircleAlert v-if="displayStatus.kind === 'streamError'" :size="16" />
        <Activity v-else-if="displayStatus.kind === 'callbackCapacityExceeded'" :size="16" />
        <Check v-else :size="16" />
      </span>
      <div>
        <p class="output-status-line">{{ displayStatus.line }}</p>
        <p class="output-status-detail">{{ displayStatus.detail }}</p>
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
        :disabled="busy || !streamControlsAvailable"
        aria-label="Volume level"
        @input="emit('gain', Number(($event.target as HTMLInputElement).value))"
      />
      <button
        class="mute-toggle"
        :class="{ muted }"
        type="button"
        :disabled="busy || !streamControlsAvailable"
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
