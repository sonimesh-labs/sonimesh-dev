import { describe, expect, it, vi } from 'vitest'
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
    targetGatedTicks: '0',
    targetForcedPlayouts: '0',
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
  it('downloads an allowlisted preview diagnostics bundle', async () => {
    const createObjectUrl = vi.fn(() => 'blob:preview-diagnostics')
    const revokeObjectUrl = vi.fn()
    vi.stubGlobal('URL', { createObjectURL: createObjectUrl, revokeObjectURL: revokeObjectUrl })
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics({ authenticatedPackets: '42' }),
        preview: true,
        diagnosticExports: [{ sequence: '3', bytes: '16384' }],
      },
    })

    await wrapper.findAll('.export-section button')[1].trigger('click')

    expect(createObjectUrl).toHaveBeenCalledTimes(1)
    const blob = (createObjectUrl.mock.calls[0] as unknown as [Blob])[0]
    const payload = JSON.parse(await blob.text()) as { format: string; metrics: StreamMetrics }
    expect(payload.format).toBe('sonimesh-preview-diagnostics/v1')
    expect(payload.metrics.authenticatedPackets).toBe('42')
    expect(click).toHaveBeenCalledTimes(1)
    expect(wrapper.text()).toContain('Preview diagnostics downloaded.')
    expect(revokeObjectUrl).toHaveBeenCalledWith('blob:preview-diagnostics')

    click.mockRestore()
    vi.unstubAllGlobals()
  })

  it('copies an allowlisted preview diagnostics bundle', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })

    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics({ authenticatedPackets: '42' }),
        preview: true,
        diagnosticExports: [{ sequence: '3', bytes: '16384' }],
      },
    })

    await wrapper.findAll('.export-section button')[2].trigger('click')

    expect(writeText).toHaveBeenCalledTimes(1)
    const payload = JSON.parse(writeText.mock.calls[0][0] as string) as { format: string; metrics: StreamMetrics }
    expect(payload.format).toBe('sonimesh-preview-diagnostics/v1')
    expect(payload.metrics.authenticatedPackets).toBe('42')
    expect(wrapper.text()).toContain('Preview diagnostics copied.')

    vi.unstubAllGlobals()
  })

  it('reports denied clipboard access without throwing', async () => {
    vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } })

    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics(),
        preview: true,
      },
    })

    await wrapper.findAll('.export-section button')[2].trigger('click')

    expect(wrapper.text()).toContain('Clipboard access was denied in this preview.')
    vi.unstubAllGlobals()
  })

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
          targetGatedTicks: '12',
          targetForcedPlayouts: '3',
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
    expect(value('Gated ticks')).toBe('12')
    expect(value('Forced playouts')).toBe('3')
  })

  it('emits a latency target change from the jitter mode control', async () => {
    const wrapper = mount(DiagnosticsPanel, {
      props: {
        metrics: metrics({ targetMinFrames: 2, targetMaxFrames: 4 }),
        busy: false,
      },
    })

    await wrapper.get('#jitter-mode').setValue('stable')

    expect(wrapper.emitted('setJitterMode')).toEqual([['stable']])
    expect((wrapper.get('#jitter-mode').element as HTMLSelectElement).value).toBe('stable')
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
