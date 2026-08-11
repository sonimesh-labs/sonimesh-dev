<script setup lang="ts">
import { Activity, Gauge, Layers, RefreshCcwDot, ShieldCheck, Waves } from '@lucide/vue'

import type { StreamMetrics } from '../types'

const props = defineProps<{
  metrics: StreamMetrics
  preview?: boolean
  busy?: boolean
}>()

const emit = defineEmits<{
  simulateMetricsBurst: []
}>()

function formatRate(sampleRateHz: number) {
  return `${(sampleRateHz / 1_000).toFixed(sampleRateHz % 1_000 === 0 ? 0 : 1)} kHz`
}

function formatRatio() {
  const { nominalInputSampleRateHz, nominalOutputSampleRateHz, driftAdjustmentPpm } = props.metrics
  if (nominalInputSampleRateHz === 0 || nominalOutputSampleRateHz === 0) return 'Unavailable'
  const nominal = nominalInputSampleRateHz / nominalOutputSampleRateHz
  return (nominal * (1 + driftAdjustmentPpm / 1_000_000)).toFixed(6)
}

function formatPpm(ppm: number) {
  return `${ppm >= 0 ? '+' : ''}${ppm} ppm`
}

function formatCounter(counter: string) {
  if (!/^\d+$/.test(counter)) return 'Unavailable'
  return counter.replace(/\B(?=(\d{3})+(?!\d))/g, ',')
}
</script>

<template>
  <section class="diagnostics panel" aria-labelledby="diagnostics-heading">
    <div class="diagnostics-heading">
      <div>
        <p class="eyebrow">{{ preview ? 'Preview observability' : 'Signal observability' }}</p>
        <h2 id="diagnostics-heading">{{ preview ? 'Simulated stream state' : 'A calm view of stream health' }}</h2>
        <p>{{ preview ? 'Synthetic preview values; not transport, audio, or hardware measurements.' : 'Transport, recovery, queue and clock data from the current bounded session.' }}</p>
      </div>
      <span class="telemetry-mark"><Activity :size="21" aria-hidden="true" /></span>
    </div>

    <div class="health-grid">
      <article class="health-card health-primary">
        <span>Jitter buffer</span>
        <strong>{{ metrics.currentJitterMs }}<small>ms</small></strong>
        <p>Current network smoothing window</p>
      </article>
      <article class="health-card">
        <span>Playout delay</span>
        <strong>{{ metrics.playoutDelayMs }}<small>ms</small></strong>
        <p>Buffered before the output callback</p>
      </article>
      <article class="health-card">
        <span>PCM queue</span>
        <strong>{{ metrics.queuedPcmBlocks }}<small>/ {{ metrics.queuedPcmHighWaterBlocks }}</small></strong>
        <p>Blocks queued / observed high water</p>
      </article>
      <article class="health-card">
        <span>Clock drift</span>
        <strong>{{ formatPpm(metrics.driftAdjustmentPpm) }}</strong>
        <p>Adaptive resampling correction</p>
      </article>
    </div>

    <div class="diagnostic-groups">
      <section class="metric-group" aria-labelledby="transport-heading">
        <div class="group-heading">
          <span class="group-icon"><ShieldCheck :size="18" aria-hidden="true" /></span>
          <div><h3 id="transport-heading">Packet integrity</h3><p>Authenticated transport counters</p></div>
        </div>
        <dl class="metric-list">
          <div><dt>Authenticated</dt><dd>{{ formatCounter(metrics.authenticatedPackets) }}</dd></div>
          <div><dt>Duplicates</dt><dd>{{ formatCounter(metrics.duplicatePackets) }}</dd></div>
          <div><dt>Rejected</dt><dd>{{ formatCounter(metrics.rejectedPackets) }}</dd></div>
          <div><dt>Late packets</dt><dd>{{ formatCounter(metrics.latePackets) }}</dd></div>
          <div><dt>Reordered</dt><dd>{{ formatCounter(metrics.reorderedPackets) }}</dd></div>
        </dl>
      </section>

      <section class="metric-group" aria-labelledby="recovery-heading">
        <div class="group-heading">
          <span class="group-icon"><Waves :size="18" aria-hidden="true" /></span>
          <div><h3 id="recovery-heading">Audio recovery</h3><p>Bounded decode and PCM events</p></div>
        </div>
        <dl class="metric-list metric-list-wide">
          <div><dt>Jitter drops</dt><dd>{{ formatCounter(metrics.jitterResyncDroppedFrames) }}</dd></div>
          <div><dt>PLC frames</dt><dd>{{ formatCounter(metrics.plcFrames) }}</dd></div>
          <div><dt>Decode errors</dt><dd>{{ formatCounter(metrics.decodeErrors) }}</dd></div>
          <div><dt>Underflows</dt><dd>{{ formatCounter(metrics.pcmUnderflows) }}</dd></div>
          <div><dt>Refill limit hits</dt><dd>{{ formatCounter(metrics.pcmRefillLimitHits) }}</dd></div>
          <div><dt>Overflows</dt><dd>{{ formatCounter(metrics.pcmOverflows) }}</dd></div>
          <div><dt>Discontinuities</dt><dd>{{ formatCounter(metrics.pcmDiscontinuities) }}</dd></div>
          <div><dt>Hard resyncs</dt><dd>{{ formatCounter(metrics.driftHardResyncs) }}</dd></div>
          <div><dt>Stream resets</dt><dd>{{ formatCounter(metrics.streamResets) }}</dd></div>
        </dl>
      </section>
    </div>

    <div class="resampling-bar" aria-label="Resampling state">
      <div><Gauge :size="17" aria-hidden="true" /><span>Resampling</span><strong>{{ formatRate(metrics.nominalInputSampleRateHz) }} → {{ formatRate(metrics.nominalOutputSampleRateHz) }}</strong></div>
      <div><RefreshCcwDot :size="17" aria-hidden="true" /><span>Effective ratio</span><strong>{{ formatRatio() }}</strong></div>
      <div><Layers :size="17" aria-hidden="true" /><span>Queue policy</span><strong>Bounded</strong></div>
    </div>

    <div v-if="preview" class="simulate-actions diagnostics-simulate" aria-label="Preview metrics simulation">
      <button class="preview-action" :disabled="busy" @click="emit('simulateMetricsBurst')">Simulate metrics burst</button>
    </div>
  </section>
</template>
