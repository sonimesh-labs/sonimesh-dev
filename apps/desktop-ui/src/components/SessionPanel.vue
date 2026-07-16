<script setup lang="ts">
import { Radio, Square } from '@lucide/vue'

import type { SessionState } from '../types'

defineProps<{
  state: SessionState
  busy: boolean
}>()

const emit = defineEmits<{
  start: []
  stop: []
}>()

const labels: Record<SessionState, string> = {
  idle: 'Idle',
  pairing: 'Pairing',
  ready: 'Ready',
  streaming: 'Streaming',
  reconnecting: 'Reconnecting',
  outputUnavailable: 'Output unavailable',
}
</script>

<template>
  <section class="panel session-panel" aria-labelledby="session-heading">
    <div class="panel-heading">
      <div>
        <p class="eyebrow">Receiver session</p>
        <h2 id="session-heading">{{ labels[state] }}</h2>
      </div>
      <span class="state-dot" :class="`state-${state}`" aria-hidden="true"></span>
    </div>
    <dl class="session-details">
      <div><dt>Transport</dt><dd>Protected UDP</dd></div>
      <div><dt>Input</dt><dd>One stream</dd></div>
      <div><dt>Mode</dt><dd>Balanced</dd></div>
    </dl>
    <div class="session-actions">
      <button v-if="state !== 'streaming'" class="primary-action" :disabled="busy || state !== 'ready'" @click="emit('start')">
        <Radio :size="17" aria-hidden="true" />
        Start stream
      </button>
      <button v-else class="danger-action" :disabled="busy" @click="emit('stop')">
        <Square :size="17" aria-hidden="true" />
        Stop stream
      </button>
    </div>
  </section>
</template>
