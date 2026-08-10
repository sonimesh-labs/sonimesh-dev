import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { DesktopSnapshot } from './types'

const desktopBridgeMock = vi.hoisted(() => ({
  createPairing: vi.fn(),
  expirePairing: vi.fn(),
  revokePairing: vi.fn(),
  selectOutput: vi.fn(),
  simulateConnectionLost: vi.fn(),
  simulateOutputLost: vi.fn(),
  simulateOutputRestored: vi.fn(),
  simulateReconnected: vi.fn(),
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
    pairing: { state: 'consumed', expiresInSeconds: null },
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
      streamResets: '3',
      nominalInputSampleRateHz: 48_000,
      nominalOutputSampleRateHz: 48_000,
      driftAdjustmentPpm: 110,
      driftHardResyncs: '1',
      currentJitterMs: 38,
      playoutDelayMs: 64,
    },
    previewAdapter: true,
  }
}

afterEach(() => {
  vi.resetAllMocks()
})

describe('App', () => {
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
})
