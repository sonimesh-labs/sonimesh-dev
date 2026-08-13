<script setup lang="ts">
import { computed } from 'vue'
import { ArrowUpRight, Radio, RefreshCw, ShieldCheck, Square } from '@lucide/vue'

import type { PairingState, SessionState } from '../types'

const props = withDefaults(defineProps<{
  state: SessionState
  pairingState: PairingState
  preview?: boolean
  busy: boolean
  reconnectRemainingMs?: number | null
}>(), {
  preview: false,
  reconnectRemainingMs: null,
})

const emit = defineEmits<{
  start: []
  stop: []
  simulateConnectionLost: []
  simulateReconnected: []
  simulateOutputLost: []
  simulateOutputRestored: []
  simulateSessionExpired: []
  simulateCommandRejection: []
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

const previewLabels: Record<SessionState, string> = {
  idle: 'Preview standing by',
  pairing: 'Preview pairing state',
  ready: 'Preview ready',
  streaming: 'Preview streaming',
  reconnecting: 'Preview reconnecting',
  outputUnavailable: 'Preview output unavailable',
}

const previewSummaries: Record<SessionState, string> = {
  idle: 'The preview adapter is waiting for a simulated session.',
  pairing: 'The preview adapter is showing a simulated pairing state; no sender is being verified.',
  ready: 'The preview adapter is ready for simulated controls; it is not connected to a sender or output device.',
  streaming: 'The preview adapter is simulating a streaming state; no audio is captured, transported, or played.',
  reconnecting: 'The preview adapter is simulating output recovery; it does not control a real audio device.',
  outputUnavailable: 'The preview adapter is showing an unavailable-output state; it does not inspect hardware.',
}

const displayLabel = computed(() => (props.preview ? previewLabels[props.state] : labels[props.state]))
const displaySummary = computed(() => (props.preview ? previewSummaries[props.state] : summaries[props.state]))
const displayControl = computed(() => (props.preview ? 'Preview state' : controlLabels[props.pairingState]))
const displayState = computed(() => (props.preview ? `Preview ${props.state}` : props.state))
const reconnectSeconds = computed(() =>
  props.reconnectRemainingMs === null ? 0 : Math.ceil(props.reconnectRemainingMs / 1_000),
)
</script>

<template>
  <section class="session-panel" :class="`session-${state}`" aria-labelledby="session-heading">
    <div class="session-topline">
      <span class="session-kicker">{{ preview ? 'Preview receiver' : 'Live receiver' }}</span>
      <span class="state-badge" :class="`state-${state}`">
        <span class="state-dot" aria-hidden="true"></span>
        {{ displayState }}
      </span>
    </div>

    <div class="session-copy">
      <h2 id="session-heading" aria-live="polite">{{ displayLabel }}</h2>
      <p>{{ displaySummary }}</p>
    </div>

    <div class="signal-stage" :class="`rail-${state}`" aria-hidden="true">
      <span v-for="index in 28" :key="index" :style="{ '--bar': index }"></span>
    </div>

    <dl class="session-details">
      <div>
        <ShieldCheck :size="17" aria-hidden="true" />
        <dt>Control</dt>
        <dd>{{ displayControl }}</dd>
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
      <div v-if="state === 'reconnecting' && reconnectRemainingMs !== null">
        <RefreshCw :size="17" aria-hidden="true" />
        <dt>Reconnect window</dt>
        <dd>{{ reconnectSeconds }}s remaining</dd>
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

    <div v-if="preview" class="simulate-actions" aria-label="Preview fault simulation">
      <button v-if="state === 'ready' || state === 'streaming'" class="preview-action" :disabled="busy" @click="emit('simulateConnectionLost')">
        Simulate connection loss
      </button>
      <button v-if="state === 'reconnecting'" class="preview-action" :disabled="busy" @click="emit('simulateReconnected')">
        Reconnect
      </button>
      <button v-if="state === 'streaming'" class="preview-action" :disabled="busy" @click="emit('simulateOutputLost')">
        Simulate output loss
      </button>
      <button v-if="state === 'outputUnavailable'" class="preview-action" :disabled="busy" @click="emit('simulateOutputRestored')">
        Restore output
      </button>
      <button v-if="pairingState === 'consumed' && ['ready', 'streaming', 'reconnecting', 'outputUnavailable'].includes(state)" class="preview-action" :disabled="busy" @click="emit('simulateSessionExpired')">
        Simulate session expiry
      </button>
      <button class="preview-action" :disabled="busy" @click="emit('simulateCommandRejection')">
        Simulate command rejection
      </button>
    </div>
  </section>
</template>
