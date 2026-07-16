<script setup lang="ts">
import { Activity, ShieldCheck, Waves } from '@lucide/vue'

import type { StreamMetrics } from '../types'

defineProps<{
  metrics: StreamMetrics
}>()
</script>

<template>
  <section class="diagnostics" aria-labelledby="diagnostics-heading">
    <div class="diagnostics-heading">
      <div>
        <p class="eyebrow">Diagnostics</p>
        <h2 id="diagnostics-heading">Live stream health</h2>
      </div>
      <Activity :size="21" aria-hidden="true" />
    </div>
    <div class="metric-grid">
      <div class="metric"><ShieldCheck :size="17" aria-hidden="true" /><span>Authenticated</span><strong>{{ metrics.authenticatedPackets.toLocaleString() }}</strong></div>
      <div class="metric"><ShieldCheck :size="17" aria-hidden="true" /><span>Rejected</span><strong>{{ metrics.rejectedPackets }}</strong></div>
      <div class="metric"><Waves :size="17" aria-hidden="true" /><span>PLC frames</span><strong>{{ metrics.plcFrames }}</strong></div>
      <div class="metric"><Waves :size="17" aria-hidden="true" /><span>Underflows</span><strong>{{ metrics.pcmUnderflows }}</strong></div>
    </div>
    <div class="latency-row">
      <span>Jitter buffer <strong>{{ metrics.currentJitterMs }} ms</strong></span>
      <span>Playout <strong>{{ metrics.playoutDelayMs }} ms</strong></span>
    </div>
  </section>
</template>
