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
    jitterFrames: 0,
    jitterHighWaterFrames: 0,
    targetMinFrames: 0,
    targetMaxFrames: 0,
    targetFrames: 0,
    targetSmoothedJitterMs: 0,
    targetRaisedSteps: 0,
    targetLoweredSteps: 0,
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
  it('labels synthetic metrics as preview values instead of measurements', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
      },
    })

    expect(wrapper.find('.eyebrow').text()).toBe('Preview observability')
    expect(wrapper.get('h2').text()).toBe('Simulated stream state')
    expect(wrapper.text()).toContain('not transport, audio, or hardware measurements')
  })

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

  it('emits a metrics-burst request from the preview-only action', async () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
        busy: false,
      },
    })

    await wrapper.get('.diagnostics-simulate button').trigger('click')

    expect(wrapper.emitted('simulateMetricsBurst')).toHaveLength(1)
  })

  it('hides the metrics-burst simulation action outside the preview adapter', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: false,
      },
    })

    expect(wrapper.find('.diagnostics-simulate').exists()).toBe(false)
  })

  it('presents the adaptive jitter target state in its own group', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics({
          jitterFrames: 3,
          jitterHighWaterFrames: 5,
          targetMinFrames: 2,
          targetMaxFrames: 4,
          targetFrames: 3,
          targetSmoothedJitterMs: 36,
          targetRaisedSteps: 1,
          targetLoweredSteps: 2,
        }),
      },
    })

    const group = wrapper.get('section[aria-labelledby="jitter-target-heading"]')
    const rows = group.findAll('.metric-list > div')
    const value = (label: string) =>
      rows.find((row) => row.get('dt').text() === label)?.get('dd').text()

    expect(group.get('h3').text()).toBe('Jitter scheduling')
    expect(value('Buffered frames')).toBe('3')
    expect(value('High water')).toBe('5')
    expect(value('Target window')).toBe('2–4')
    expect(value('Current target')).toBe('3')
    expect(value('Smoothed jitter')).toBe('36 ms')
    expect(value('Target raised')).toBe('1')
    expect(value('Target lowered')).toBe('2')
  })

  it('emits a diagnostic-export request from the preview-only action', async () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
        busy: false,
      },
    })

    await wrapper.get('.export-section button').trigger('click')

    expect(wrapper.emitted('exportDiagnostics')).toHaveLength(1)
  })

  it('renders bounded diagnostic export receipts with formatted bytes', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
        diagnosticExports: [
          { sequence: '3', bytes: '16384' },
          { sequence: '2', bytes: '16384' },
          { sequence: '1', bytes: '16384' },
          { sequence: '0', bytes: '16384' },
        ],
      },
    })

    const receipts = wrapper.findAll('.export-receipts li')
    expect(receipts.map((row) => row.text())).toEqual([
      'Export #3 · 16.0 KiB',
      'Export #2 · 16.0 KiB',
      'Export #1 · 16.0 KiB',
      'Export #0 · 16.0 KiB',
    ])
  })

  it('shows an empty export state before any preview export is issued', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
        diagnosticExports: [],
      },
    })

    expect(wrapper.find('.export-receipts').exists()).toBe(false)
    expect(wrapper.text()).toContain('No diagnostic exports issued in this preview session.')
  })

  it('hides the diagnostic-export action outside the preview adapter', () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: false,
      },
    })

    expect(wrapper.find('.export-section').exists()).toBe(false)
  })
})
