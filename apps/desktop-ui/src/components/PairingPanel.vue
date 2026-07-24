<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Clock3, QrCode, X } from '@lucide/vue'

import type { PairingStatus } from '../types'

const props = defineProps<{
  pairing: PairingStatus
  busy: boolean
}>()

const emit = defineEmits<{
  create: []
  expire: []
}>()

const remainingSeconds = ref<number | null>(null)
let countdownTimer: number | undefined

function clearCountdown() {
  if (countdownTimer !== undefined) {
    window.clearInterval(countdownTimer)
    countdownTimer = undefined
  }
}

function beginCountdown(pairing: PairingStatus) {
  clearCountdown()
  remainingSeconds.value = pairing.state === 'active' ? pairing.expiresInSeconds : null

  if (remainingSeconds.value === null) return

  countdownTimer = window.setInterval(() => {
    if (remainingSeconds.value === null) {
      clearCountdown()
      return
    }
    remainingSeconds.value = Math.max(0, remainingSeconds.value - 1)
    if (remainingSeconds.value === 0) {
      clearCountdown()
      emit('expire')
    }
  }, 1_000)
}

const pairingMessage = computed(() => {
  switch (props.pairing.state) {
    case 'active':
      return remainingSeconds.value === null ? 'Invitation is open' : `${remainingSeconds.value}s remaining`
    case 'consumed':
      return 'Sender verified'
    case 'expired':
      return 'Invitation expired'
    case 'inactive':
      return 'No open invitation'
  }
})

const pairingHint = computed(() => {
  switch (props.pairing.state) {
    case 'active':
      return 'Keep this window open while the sender verifies the invitation.'
    case 'consumed':
      return 'The invitation was used once and cannot be replayed.'
    case 'expired':
      return 'Create a fresh invitation when the sender is ready.'
    case 'inactive':
      return 'Create a short-lived invitation to trust a new sender.'
  }
})

watch(() => props.pairing, beginCountdown, { immediate: true })
onUnmounted(clearCountdown)
</script>

<template>
  <section class="panel utility-panel pairing-panel" aria-labelledby="pairing-heading">
    <div class="panel-heading">
      <div class="panel-icon"><QrCode :size="19" aria-hidden="true" /></div>
      <div>
        <p class="eyebrow">Private pairing</p>
        <h2 id="pairing-heading">Trust a sender</h2>
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
      <button class="secondary-action" :disabled="busy || pairing.state === 'active'" @click="emit('create')">
        Create invitation
      </button>
      <button v-if="pairing.state === 'active'" class="icon-action subtle" title="Expire invitation" aria-label="Expire invitation" :disabled="busy" @click="emit('expire')">
        <X :size="18" aria-hidden="true" />
      </button>
    </div>
  </section>
</template>
