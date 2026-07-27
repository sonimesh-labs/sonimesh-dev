<script setup lang="ts">
import { ArrowUpRight, Radio, ShieldCheck, Square } from '@lucide/vue'

import type { PairingState, SessionState } from '../types'

const props = defineProps<{
  state: SessionState
  pairingState: PairingState
  preview: boolean
  busy: boolean
}>()

const emit = defineEmits<{
  start: []
  stop: []
}>()

const labels: Record<SessionState, string> = {
  idle: 'Standing by',
  pairing: 'Pairing sender',
  ready: 'Ready to listen',
  streaming: 'Now streaming',
  reconnecting: 'Reconnecting',
  outputUnavailable: 'Output unavailable',
}

const summaries: Record<SessionState, string> = {
  idle: 'The receiver is waiting for a trusted session.',
  pairing: 'Verifying the current pairing invitation.',
  ready: 'Your sender is trusted and the selected output is available.',
  streaming: 'Audio is moving privately across your local network.',
  reconnecting: 'Keeping the session while the output path recovers.',
  outputUnavailable: 'Choose or reconnect an output device to continue.',
}

const controlLabels: Record<PairingState, string> = {
  inactive: 'Not authenticated',
  active: 'Verification pending',
  consumed: 'Authenticated',
  expired: 'Not authenticated',
  revoked: 'Revoked',
}
</script>

<template>
  <section class="session-panel" :class="`session-${state}`" aria-labelledby="session-heading">
    <div class="session-topline">
      <span class="session-kicker">Live receiver</span>
      <span class="state-badge" :class="`state-${state}`">
        <span class="state-dot" aria-hidden="true"></span>
        {{ state }}
      </span>
    </div>

    <div class="session-copy">
      <h2 id="session-heading" aria-live="polite">{{ labels[state] }}</h2>
      <p>{{ summaries[state] }}</p>
    </div>

    <div class="signal-stage" :class="`rail-${state}`" aria-hidden="true">
      <span v-for="index in 28" :key="index" :style="{ '--bar': index }"></span>
    </div>

    <dl class="session-details">
      <div>
        <ShieldCheck :size="17" aria-hidden="true" />
        <dt>Control</dt>
        <dd>{{ controlLabels[pairingState] }}</dd>
      </div>
      <div>
        <Radio :size="17" aria-hidden="true" />
        <dt>Input</dt>
        <dd>Single stream</dd>
      </div>
      <div>
        <ArrowUpRight :size="17" aria-hidden="true" />
        <dt>Mode</dt>
        <dd>{{ preview ? 'Preview adapter' : 'Receiver' }}</dd>
      </div>
    </dl>

    <div class="session-actions">
      <button v-if="state !== 'streaming'" class="primary-action" :disabled="busy || state !== 'ready' || pairingState !== 'consumed'" @click="emit('start')">
        <Radio :size="17" aria-hidden="true" />
        <span>{{ preview ? 'Start listening preview' : 'Start listening' }}</span>
        <ArrowUpRight :size="17" aria-hidden="true" />
      </button>
      <button v-else class="danger-action" :disabled="busy" @click="emit('stop')">
        <Square :size="16" aria-hidden="true" />
        <span>{{ preview ? 'Stop listening preview' : 'Stop listening' }}</span>
      </button>
    </div>
  </section>
</template>
