<script setup lang="ts">
import { Activity, Gauge, Layers, RefreshCcwDot, ShieldCheck, Waves } from '@lucide/vue'

import type { StreamMetrics } from '../types'

const props = defineProps<{
  metrics: StreamMetrics
}>()

function formatRate(sampleRateHz: number) {
  return `${(sampleRateHz / 1_000).toFixed(sampleRateHz % 1_000 === 0 ? 0 : 1)} kHz`
}

function formatRatio() {
  const { nominalInputSampleRateHz, nominalOutputSampleRateHz, driftAdjustmentPpm } = props.metrics
  if (nominalInputSampleRateHz === 0 || nominalOutputSampleRateHz === 0) {
    return 'Unavailable'
  }
  const nominal = nominalInputSampleRateHz / nominalOutputSampleRateHz
  return (nominal * (1 + driftAdjustmentPpm / 1_000_000)).toFixed(6)
}

function formatPpm(ppm: number) {
  return `${ppm >= 0 ? '+' : ''}${ppm} ppm`
}
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
      <div class="metric"><Activity :size="17" aria-hidden="true" /><span>Reordered</span><strong>{{ metrics.reorderedPackets }}</strong></div>
      <div class="metric"><RefreshCcwDot :size="17" aria-hidden="true" /><span>Jitter drops</span><strong>{{ metrics.jitterResyncDroppedFrames }}</strong></div>
      <div class="metric"><Waves :size="17" aria-hidden="true" /><span>PLC frames</span><strong>{{ metrics.plcFrames }}</strong></div>
      <div class="metric"><Waves :size="17" aria-hidden="true" /><span>Decode errors</span><strong>{{ metrics.decodeErrors }}</strong></div>
      <div class="metric"><Waves :size="17" aria-hidden="true" /><span>Underflows</span><strong>{{ metrics.pcmUnderflows }}</strong></div>
      <div class="metric"><RefreshCcwDot :size="17" aria-hidden="true" /><span>Discontinuities</span><strong>{{ metrics.pcmDiscontinuities }}</strong></div>
      <div class="metric"><RefreshCcwDot :size="17" aria-hidden="true" /><span>Hard resyncs</span><strong>{{ metrics.driftHardResyncs }}</strong></div>
    </div>
    <div class="resampling-row" aria-label="PCM queue and resampling state">
      <div><Layers :size="17" aria-hidden="true" /><span>PCM queue</span><strong>{{ metrics.queuedPcmBlocks }} / {{ metrics.queuedPcmHighWaterBlocks }} blocks</strong></div>
      <div><Gauge :size="17" aria-hidden="true" /><span>Resampling</span><strong>{{ formatRate(metrics.nominalInputSampleRateHz) }} to {{ formatRate(metrics.nominalOutputSampleRateHz) }}</strong></div>
      <div><Gauge :size="17" aria-hidden="true" /><span>Drift</span><strong>{{ formatPpm(metrics.driftAdjustmentPpm) }} · ratio {{ formatRatio() }}</strong></div>
    </div>
    <div class="latency-row">
      <span>Jitter buffer <strong>{{ metrics.currentJitterMs }} ms</strong></span>
      <span>Playout <strong>{{ metrics.playoutDelayMs }} ms</strong></span>
    </div>
  </section>
</template>
