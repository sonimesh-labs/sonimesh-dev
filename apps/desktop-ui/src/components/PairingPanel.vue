<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Clock3, QrCode, ShieldX, X } from '@lucide/vue'

import type { PairingStatus } from '../types'

const props = defineProps<{
  pairing: PairingStatus
  busy: boolean
  preview?: boolean
}>()

const emit = defineEmits<{
  create: []
  expire: []
  refresh: []
  revoke: []
}>()

const remainingSeconds = ref<number | null>(null)
let countdownTimer: number | undefined
let expiryStatusRefreshRequested = false

function clearCountdown() {
  if (countdownTimer !== undefined) {
    window.clearInterval(countdownTimer)
    countdownTimer = undefined
  }
}

function refreshExpiryStatusOnce() {
  if (!expiryStatusRefreshRequested) {
    expiryStatusRefreshRequested = true
    emit('refresh')
  }
}

function countdownReachedDeadline(deadlineMillis: number): boolean {
  const secondsRemaining = Math.max(0, Math.ceil((deadlineMillis - Date.now()) / 1_000))
  remainingSeconds.value = secondsRemaining
  if (secondsRemaining !== 0) return false

  refreshExpiryStatusOnce()
  return true
}

function beginCountdown(pairing: PairingStatus) {
  clearCountdown()
  if (pairing.state !== 'active') {
    expiryStatusRefreshRequested = false
    remainingSeconds.value = null
    return
  }

  const expiresInSeconds = pairing.expiresInSeconds
  remainingSeconds.value = expiresInSeconds
  if (expiresInSeconds === null) return

  const deadlineMillis = Date.now() + expiresInSeconds * 1_000
  if (countdownReachedDeadline(deadlineMillis)) return

  countdownTimer = window.setInterval(() => {
    if (countdownReachedDeadline(deadlineMillis)) {
      clearCountdown()
    }
  }, 1_000)
}

const pairingMessage = computed(() => {
  if (props.preview) {
    switch (props.pairing.state) {
      case 'active':
        return remainingSeconds.value === null ? 'Preview invitation is open' : `Preview · ${remainingSeconds.value}s remaining`
      case 'consumed':
        return 'Preview trust state'
      case 'expired':
        return 'Preview invitation expired'
      case 'inactive':
        return 'No preview invitation'
      case 'revoked':
        return 'Preview trust revoked'
    }
  }

  switch (props.pairing.state) {
    case 'active':
      return remainingSeconds.value === null ? 'Invitation is open' : `${remainingSeconds.value}s remaining`
    case 'consumed':
      return 'Sender verified'
    case 'expired':
      return 'Invitation expired'
    case 'inactive':
      return 'No open invitation'
    case 'revoked':
      return 'Sender trust revoked'
  }
})

const pairingHint = computed(() => {
  if (props.preview) {
    return 'The preview adapter changes local UI state only; it does not create or verify a real invitation.'
  }

  switch (props.pairing.state) {
    case 'active':
      return 'Keep this window open while the sender verifies the invitation.'
    case 'consumed':
      return 'The invitation was used once and cannot be replayed.'
    case 'expired':
      return 'Create a fresh invitation when the sender is ready.'
    case 'inactive':
      return 'Create a short-lived invitation to trust a new sender.'
    case 'revoked':
      return 'Create a fresh invitation before trusting this sender again.'
  }
})

const pairingEyebrow = computed(() => (props.preview ? 'Pairing preview' : 'Private pairing'))
const pairingHeading = computed(() => (props.preview ? 'Simulate sender trust' : 'Trust a sender'))
const createButtonLabel = computed(() => (props.preview ? 'Create preview invitation' : 'Create invitation'))

watch(() => props.pairing, beginCountdown, { immediate: true })
onUnmounted(clearCountdown)
</script>

<template>
  <section class="panel utility-panel pairing-panel" aria-labelledby="pairing-heading">
    <div class="panel-heading">
      <div class="panel-icon"><QrCode :size="19" aria-hidden="true" /></div>
      <div>
        <p class="eyebrow">{{ pairingEyebrow }}</p>
        <h2 id="pairing-heading">{{ pairingHeading }}</h2>
      </div>
    </div>

    <div class="pairing-callout" :class="`pairing-${pairing.state}`">
      <span class="pairing-orb" aria-hidden="true"></span>
      <div>
        <strong>{{ pairingMessage }}</strong>
        <p>{{ pairingHint }}</p>
      </div>
      <Clock3 v-if="pairing.state === 'active'" :size="17" aria-hidden="true" />
    </div>

    <div class="inline-actions">
      <button class="secondary-action" :disabled="busy || pairing.state === 'active' || pairing.state === 'consumed'" @click="emit('create')">
        {{ createButtonLabel }}
      </button>
      <button v-if="pairing.state === 'active'" class="icon-action subtle" :title="preview ? 'Expire preview invitation' : 'Expire invitation'" :aria-label="preview ? 'Expire preview invitation' : 'Expire invitation'" :disabled="busy" @click="emit('expire')">
        <X :size="18" aria-hidden="true" />
      </button>
      <button v-if="pairing.state === 'consumed'" class="icon-action subtle" :title="preview ? 'Revoke preview trust' : 'Revoke trust'" :aria-label="preview ? 'Revoke preview trust' : 'Revoke trust'" :disabled="busy" @click="emit('revoke')">
        <ShieldX :size="18" aria-hidden="true" />
      </button>
    </div>
  </section>
</template>
