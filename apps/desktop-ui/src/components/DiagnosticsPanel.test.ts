import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'

import DiagnosticsPanel from './DiagnosticsPanel.vue'
import type { StreamMetrics } from '../types'

function metrics(overrides: Partial<StreamMetrics> = {}): StreamMetrics {
  return {
    authenticatedPackets: '0',
    duplicatePackets: '0',
    rejectedPackets: '0',
    latePackets: '0',
    reorderedPackets: '0',
    jitterResyncDroppedFrames: '0',
    plcFrames: '0',
    decodeErrors: '0',
    pcmUnderflows: '0',
    pcmRefillLimitHits: '0',
    pcmOverflows: '0',
    pcmDiscontinuities: '0',
    queuedPcmBlocks: 0,
    queuedPcmHighWaterBlocks: 0,
    streamResets: '0',
    nominalInputSampleRateHz: 48_000,
    nominalOutputSampleRateHz: 48_000,
    driftAdjustmentPpm: 0,
    driftHardResyncs: '0',
    currentJitterMs: 0,
    playoutDelayMs: 0,
    ...overrides,
  }
}

describe('DiagnosticsPanel', () => {
  it('formats u64 counters as decimal strings without precision loss and rejects malformed bridge values', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics({
          authenticatedPackets: '18446744073709551615',
          duplicatePackets: '1e6',
        }),
      },
    })

    const metricRows = wrapper.findAll('.metric-list > div')
    const authenticated = metricRows.find((row) => row.get('dt').text() === 'Authenticated')
    const duplicates = metricRows.find((row) => row.get('dt').text() === 'Duplicates')

    expect(authenticated?.get('dd').text()).toBe('18,446,744,073,709,551,615')
    expect(duplicates?.get('dd').text()).toBe('Unavailable')
  })
})
