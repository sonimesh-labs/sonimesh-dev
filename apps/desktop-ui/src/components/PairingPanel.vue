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

  if (remainingSeconds.value === null) {
    return
  }

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
      return remainingSeconds.value === null ? 'Invitation active' : remainingSeconds.value + 's remaining'
    case 'consumed':
      return 'Verified invitation consumed'
    case 'expired':
      return 'Expired'
    case 'inactive':
      return 'No active invitation'
  }
})

watch(() => props.pairing, beginCountdown, { immediate: true })
onUnmounted(clearCountdown)
</script>

<template>
  <section class="panel" aria-labelledby="pairing-heading">
    <div class="panel-heading compact-heading">
      <div>
        <p class="eyebrow">Trust</p>
        <h2 id="pairing-heading">Pairing invitation</h2>
      </div>
      <QrCode :size="21" aria-hidden="true" />
    </div>
    <p class="pairing-state" :class="`pairing-${pairing.state}`">
      <Clock3 v-if="pairing.state === 'active'" :size="15" aria-hidden="true" />
      <span>{{ pairingMessage }}</span>
    </p>
    <p class="pairing-hint">Pairing state is control-plane only.</p>
    <div class="inline-actions">
      <button class="secondary-action" :disabled="busy || pairing.state === 'active'" @click="emit('create')">Create invitation</button>
      <button v-if="pairing.state === 'active'" class="icon-action" title="Expire invitation" aria-label="Expire invitation" :disabled="busy" @click="emit('expire')">
        <X :size="17" aria-hidden="true" />
      </button>
    </div>
  </section>
</template>
