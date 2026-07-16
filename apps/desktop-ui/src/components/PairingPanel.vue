<script setup lang="ts">
import { Clock3, QrCode, X } from '@lucide/vue'

import type { PairingStatus } from '../types'

defineProps<{
  pairing: PairingStatus
  busy: boolean
}>()

const emit = defineEmits<{
  create: []
  expire: []
}>()
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
      {{ pairing.state === 'active' ? `${pairing.expiresInSeconds}s remaining` : pairing.state === 'expired' ? 'Expired' : 'No active invitation' }}
    </p>
    <div class="inline-actions">
      <button class="secondary-action" :disabled="busy || pairing.state === 'active'" @click="emit('create')">Create invitation</button>
      <button v-if="pairing.state === 'active'" class="icon-action" title="Expire invitation" :disabled="busy" @click="emit('expire')">
        <X :size="17" aria-hidden="true" />
      </button>
    </div>
  </section>
</template>
