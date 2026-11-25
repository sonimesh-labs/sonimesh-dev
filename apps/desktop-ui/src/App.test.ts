import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { DesktopSnapshot } from './types'

const desktopBridgeMock = vi.hoisted(() => ({
  createPairing: vi.fn(),
  expirePairing: vi.fn(),
  exportDiagnostics: vi.fn(),
  revokePairing: vi.fn(),
  resetPreview: vi.fn(),
  selectOutput: vi.fn(),
  refreshOutputDevices: vi.fn(),
  testOutput: vi.fn(),
  setGain: vi.fn(),
  setMuted: vi.fn(),
  simulateCommandRejection: vi.fn(),
  simulateConnectionLost: vi.fn(),
  simulateMetricsBurst: vi.fn(),
  simulateOutputLost: vi.fn(),
  simulateOutputRestored: vi.fn(),
  simulatePairingConsumed: vi.fn(),
  simulateReconnected: vi.fn(),
  simulateSessionExpired: vi.fn(),
  snapshot: vi.fn(),
  startStream: vi.fn(),
  stopStream: vi.fn(),
}))

vi.mock('./bridge', () => ({
  desktopBridge: desktopBridgeMock,
}))

import App from './App.vue'

function deferred<T>() {
  let resolve: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return {
    promise,
    resolve: (value: T) => resolve(value),
  }
}

function previewSnapshot(): DesktopSnapshot {
  return {
    session: 'ready',
    outputDevices: [
      { id: 'default', label: 'Synthetic output', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ],
    selectedOutputId: 'default',
    gain: 1,
    muted: false,
    output: {
      selectionRetained: true,
      streamActive: false,
      faultReported: false,
      health: 'healthy',
    },
    pairing: { state: 'consumed', expiresInSeconds: null },
    reconnectRemainingMs: null,
    metrics: {
      authenticatedPackets: '12844',
      duplicatePackets: '6',
      rejectedPackets: '3',
      latePackets: '9',
      reorderedPackets: '11',
      jitterResyncDroppedFrames: '4',
      plcFrames: '7',
      decodeErrors: '1',
      pcmUnderflows: '0',
      pcmRefillLimitHits: '1',
      pcmOverflows: '2',
      pcmDiscontinuities: '2',
      queuedPcmBlocks: 3,
      queuedPcmHighWaterBlocks: 5,
      jitterFrames: 3,
      jitterHighWaterFrames: 5,
      targetMinFrames: 2,
      targetMaxFrames: 4,
      targetFrames: 3,
      targetSmoothedJitterMs: 36,
      targetRaisedSteps: 0,
      targetLoweredSteps: 2,
      targetGatedTicks: '12',
      targetForcedPlayouts: '1',
      streamResets: '3',
      nominalInputSampleRateHz: 48_000,
      nominalOutputSampleRateHz: 48_000,
      driftAdjustmentPpm: 110,
      driftHardResyncs: '1',
      currentJitterMs: 38,
      playoutDelayMs: 64,
    },
    replies: [],
    diagnosticExports: [],
    previewAdapter: true,
  }
}

afterEach(() => {
  vi.resetAllMocks()
})

describe('App', () => {
  it('sends the follow-default request and displays the shell-confirmed output', async () => {
    const initial = {
      ...previewSnapshot(),
      previewAdapter: false,
      output: { ...previewSnapshot().output, followsSystemDefault: false },
    }
    const following = { ...initial, output: { ...initial.output, followsSystemDefault: true } }
    desktopBridgeMock.snapshot.mockResolvedValueOnce(initial)
    desktopBridgeMock.selectOutput.mockResolvedValueOnce(following)
    const wrapper = mount(App)
    await flushPromises()
    await wrapper.findAll('button').find((button) => button.text() === 'Follow system default')!.trigger('click')
    await flushPromises()
    expect(desktopBridgeMock.selectOutput).toHaveBeenCalledWith(null)
    expect(wrapper.text()).toContain('Using the system default when available.')
    wrapper.unmount()
  })

  it('offers device retry after an initial snapshot failure and displays recovered output', async () => {
    desktopBridgeMock.snapshot.mockRejectedValue(new Error('enumeration failed'))
    const wrapper = mount(App)
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('desktop control service')
    const retry = wrapper.findAll('button').find(button => button.text() === 'Retry output devices')!
    expect(retry.exists()).toBe(true)

    desktopBridgeMock.refreshOutputDevices.mockResolvedValueOnce(previewSnapshot())
    await retry.trigger('click')
    await flushPromises()
    expect(desktopBridgeMock.refreshOutputDevices).toHaveBeenCalledOnce()
    expect(wrapper.text()).toContain('Synthetic output')
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('uses neutral copy before the first authoritative snapshot arrives', async () => {
    const pendingSnapshot = deferred<DesktopSnapshot>()
    desktopBridgeMock.snapshot.mockImplementationOnce(() => pendingSnapshot.promise)
    const wrapper = mount(App)

    expect(wrapper.get('h1').text()).toContain('Connecting to desktop')
    expect(wrapper.get('.topbar-copy').text()).toBe('Waiting for authoritative desktop state.')
    expect(wrapper.text()).not.toContain('Route your phone audio')

    pendingSnapshot.resolve(previewSnapshot())
    await flushPromises()
  })

  it('simulates connection loss from the preview session panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulateConnectionLost.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'reconnecting',
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate connection loss')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.simulateConnectionLost).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.session-chip').text()).toBe('Preview reconnecting')
  })

  it('revokes synthetic preview trust from the pairing panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.revokePairing.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'idle',
      pairing: { state: 'revoked', expiresInSeconds: null },
    })

    await wrapper.get('button[aria-label="Revoke preview trust"]').trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.revokePairing).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.session-chip').text()).toBe('Preview idle')
  })

  it('keeps the preview snapshot from looking like live audio transport', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    expect(wrapper.get('h1').text()).toContain('Preview the receiver')
    expect(wrapper.get('.session-chip').text()).toBe('Preview ready')
    expect(wrapper.get('.topbar-copy').text()).toContain('no audio is captured, transported, or played')
    expect(wrapper.get('#session-heading').text()).toBe('Preview ready')
    expect(wrapper.get('#output-heading').text()).toBe('Simulated output')
    expect(wrapper.get('#pairing-heading').text()).toBe('Simulate sender trust')
    expect(wrapper.text()).toContain('Preview observability')
    expect(wrapper.text()).not.toContain('Route your phone audio')
  })

  it('exposes real output selection while disabling receiver actions until services are connected', async () => {
    const shellSnapshot = {
      ...previewSnapshot(),
      session: 'idle' as const,
      outputDevices: [
        { id: 'wasapi:headphones', label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: true },
      ],
      selectedOutputId: 'wasapi:headphones',
      previewAdapter: false,
      capabilities: {
        receiver: false,
        pairing: false,
        streamControls: false,
        diagnostics: false,
        outputTest: true,
      },
    }
    desktopBridgeMock.snapshot.mockResolvedValueOnce(shellSnapshot)
    const wrapper = mount(App)
    await flushPromises()

    expect(wrapper.get('h1').text()).toContain('Choose an output')
    expect(wrapper.get('.session-copy p').text()).toContain('not connected yet')
    expect((wrapper.get('#output-device').element as HTMLSelectElement).disabled).toBe(false)
    expect(wrapper.get('#output-device option').text()).toContain('Headphones')
    expect((wrapper.get('.output-test-button').element as HTMLButtonElement).disabled).toBe(false)
    expect((wrapper.get('#output-volume').element as HTMLInputElement).disabled).toBe(true)
    expect((wrapper.get('.inline-actions button.secondary-action').element as HTMLButtonElement).disabled).toBe(true)
    expect(wrapper.find('#diagnostics').exists()).toBe(false)

    desktopBridgeMock.testOutput.mockResolvedValueOnce(undefined)
    await wrapper.get('.output-test-button').trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.testOutput).toHaveBeenCalledOnce()
    expect(wrapper.get('.output-test-status').text()).toBe('Test tone completed on the selected output.')
  })
  it('simulates the sender consuming the preview invitation from the pairing panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'pairing',
      pairing: { state: 'active', expiresInSeconds: 300 },
    })
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulatePairingConsumed.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'ready',
      pairing: { state: 'consumed', expiresInSeconds: null },
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate sender verification')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.simulatePairingConsumed).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.session-chip').text()).toBe('Preview ready')
  })

  it('simulates a metrics burst from the diagnostics panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulateMetricsBurst.mockResolvedValueOnce({
      ...previewSnapshot(),
      metrics: {
        ...previewSnapshot().metrics,
        authenticatedPackets: '12924',
        currentJitterMs: 52,
      },
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate metrics burst')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.simulateMetricsBurst).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.health-card strong').text()).toBe('52ms')
    expect(wrapper.text()).toContain('12,924')
  })

  it('shows the command error banner after a simulated rejection and clears it on the next command', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulateCommandRejection.mockRejectedValueOnce(new Error('rejected'))

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate command rejection')!
      .trigger('click')
    await flushPromises()

    expect(wrapper.get('.command-error').text()).toBe('The desktop control service did not accept this command.')

    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    await wrapper.get('.icon-action.refresh').trigger('click')
    await flushPromises()

    expect(wrapper.find('.command-error').exists()).toBe(false)
  })

  it('keeps output selection enabled while a background snapshot is pending', async () => {
    vi.useFakeTimers()
    const pendingSnapshot = deferred<DesktopSnapshot>()
    const snapshotWithAlternate: DesktopSnapshot = {
      ...previewSnapshot(),
      outputDevices: [
        ...previewSnapshot().outputDevices,
        { id: 'alternate', label: 'Alternate output', sampleRateHz: 48_000, channels: 2, isDefault: false },
      ],
    }
    desktopBridgeMock.snapshot
      .mockResolvedValueOnce(snapshotWithAlternate)
      .mockReturnValueOnce(pendingSnapshot.promise)
    desktopBridgeMock.selectOutput.mockResolvedValueOnce({
      ...snapshotWithAlternate,
      selectedOutputId: 'alternate',
    })
    const wrapper = mount(App)
    await flushPromises()

    vi.advanceTimersByTime(1_000)
    await flushPromises()
    const outputSelect = wrapper.get('#output-device')
    expect((outputSelect.element as HTMLSelectElement).disabled).toBe(false)

    await outputSelect.setValue('alternate')
    await flushPromises()
    expect(desktopBridgeMock.selectOutput).toHaveBeenCalledWith('alternate')

    pendingSnapshot.resolve(previewSnapshot())
    await flushPromises()
    expect(wrapper.get('#output-device').element).toHaveProperty('value', 'alternate')
    vi.useRealTimers()
  })

  it('refreshes the desktop output inventory on request', async () => {
    const liveSnapshot: DesktopSnapshot = {
      ...previewSnapshot(),
      previewAdapter: false,
      capabilities: {
        receiver: false,
        pairing: false,
        streamControls: false,
        diagnostics: false,
        outputTest: true,
      },
    }
    desktopBridgeMock.snapshot.mockResolvedValueOnce(liveSnapshot)
    desktopBridgeMock.refreshOutputDevices.mockResolvedValueOnce({
      ...liveSnapshot,
      outputDevices: [
        { id: 'default', label: 'System default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
      ],
    })
    const wrapper = mount(App)
    await flushPromises()

    await wrapper.get('[aria-label="Refresh output devices"]').trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.refreshOutputDevices).toHaveBeenCalledOnce()
    expect(wrapper.findAll('#output-device option')).toHaveLength(1)
  })

  it('simulates session expiry from the preview session panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulateSessionExpired.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'idle',
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate session expiry')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.simulateSessionExpired).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.session-chip').text()).toBe('Preview idle')
  })

  it('resets the preview state from the topbar', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.resetPreview.mockResolvedValueOnce(previewSnapshot())

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Reset preview')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.resetPreview).toHaveBeenCalledTimes(1)
    expect(wrapper.get('.health-card strong').text()).toBe('38ms')
    expect(wrapper.text()).toContain('12,844')
  })

  it('publishes a volume change from the output panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.setGain.mockResolvedValueOnce({ ...previewSnapshot(), gain: 0.5 })

    await wrapper.get('input[type="range"]').setValue('0.5')
    await flushPromises()

    expect(desktopBridgeMock.setGain).toHaveBeenCalledWith(0.5)
    expect(wrapper.get('.volume-value').text()).toBe('50%')
  })

  it('surfaces a simulated output fault in the output panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce({ ...previewSnapshot(), session: 'streaming' })
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.simulateOutputLost.mockResolvedValueOnce({
      ...previewSnapshot(),
      session: 'outputUnavailable',
      output: { selectionRetained: true, streamActive: false, faultReported: true, health: 'streamError' },
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate output loss')!
      .trigger('click')
    await flushPromises()

    expect(wrapper.get('.session-chip').text()).toBe('Preview outputUnavailable')
    expect(wrapper.get('.output-status').text()).toContain('Stream error')
    expect(wrapper.get('.output-status').text()).toContain('stream fault was reported')
  })

  it('renders the recent control replies from the preview snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce({
      ...previewSnapshot(),
      replies: [
        { kind: 'streamStatus', requestId: 1, state: 'streaming', detail: 'stream started' },
        { kind: 'error', requestId: 2, code: 'retryLater', retryable: true, detail: 'preview command rejection' },
      ],
    })
    const wrapper = mount(App)
    await flushPromises()

    expect(wrapper.get('#replies-heading').text()).toContain('Recent control replies')
    expect(wrapper.text()).toContain('stream started')
    expect(wrapper.text()).toContain('retryLater · retryable')
  })

  it('issues a preview diagnostic export from the diagnostics panel', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(previewSnapshot())
    const wrapper = mount(App)
    await flushPromises()

    desktopBridgeMock.exportDiagnostics.mockResolvedValueOnce({
      ...previewSnapshot(),
      diagnosticExports: [{ sequence: '0', bytes: '16384' }],
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Export diagnostics')!
      .trigger('click')
    await flushPromises()

    expect(desktopBridgeMock.exportDiagnostics).toHaveBeenCalledTimes(1)
    expect(wrapper.text()).toContain('Export #0')
    expect(wrapper.text()).toContain('16.0 KiB')
  })


})
