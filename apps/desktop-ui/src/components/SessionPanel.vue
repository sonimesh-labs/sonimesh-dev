<script setup lang="ts">
import { ArrowUpRight, Radio, ShieldCheck, Square } from '@lucide/vue'

import type { SessionState } from '../types'

const props = defineProps<{
  state: SessionState
  preview: boolean
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

const summaries: Record<SessionState, string> = {
  idle: 'Awaiting a trusted receiver session.',
  pairing: 'Verifying the current pairing invitation.',
  ready: 'Receiver control is ready to start the negotiated stream.',
  streaming: 'The control service reports a stream for the selected output.',
  reconnecting: 'The receiver is recovering its output path.',
  outputUnavailable: 'Choose or reconnect an output device before streaming.',
}
</script>

<template>
  <section class="panel session-panel" aria-labelledby="session-heading">
    <div class="panel-heading">
      <div>
        <p class="eyebrow">Receiver session</p>
        <h2 id="session-heading" aria-live="polite">{{ labels[state] }}</h2>
      </div>
      <span class="state-dot" :class="`state-${state}`" aria-hidden="true"></span>
    </div>
    <p class="session-summary">{{ summaries[state] }}</p>
    <div class="signal-rail" :class="`rail-${state}`" aria-hidden="true">
      <span v-for="index in 18" :key="index"></span>
    </div>
    <dl class="session-details">
      <div><ShieldCheck :size="16" aria-hidden="true" /><dt>Control</dt><dd>Low-rate bridge</dd></div>
      <div><Radio :size="16" aria-hidden="true" /><dt>Input</dt><dd>One stream</dd></div>
      <div><ArrowUpRight :size="16" aria-hidden="true" /><dt>Mode</dt><dd>{{ preview ? 'Preview' : 'Receiver' }}</dd></div>
    </dl>
    <div class="session-actions">
      <button v-if="state !== 'streaming'" class="primary-action" :disabled="busy || state !== 'ready'" @click="emit('start')">
        <Radio :size="17" aria-hidden="true" />
        {{ preview ? 'Start preview' : 'Start receiver' }}
      </button>
      <button v-else class="danger-action" :disabled="busy" @click="emit('stop')">
        <Square :size="17" aria-hidden="true" />
        {{ preview ? 'Stop preview' : 'Stop stream' }}
      </button>
    </div>
  </section>
</template>
