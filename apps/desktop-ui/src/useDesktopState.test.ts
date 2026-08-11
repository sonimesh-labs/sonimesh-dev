import { defineComponent } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'

import type { DesktopSnapshot } from './types'

const desktopBridgeMock = vi.hoisted(() => ({
  createPairing: vi.fn(),
  expirePairing: vi.fn(),
  revokePairing: vi.fn(),
  resetPreview: vi.fn(),
  selectOutput: vi.fn(),
  simulateCommandRejection: vi.fn(),
  simulateConnectionLost: vi.fn(),
  simulateMetricsBurst: vi.fn(),
  simulateOutputLost: vi.fn(),
  simulateOutputRestored: vi.fn(),
  simulatePairingConsumed: vi.fn(),
  simulateReconnected: vi.fn(),
  snapshot: vi.fn(),
  startStream: vi.fn(),
  stopStream: vi.fn(),
}))

vi.mock('./bridge', () => ({
  desktopBridge: desktopBridgeMock,
}))

import { useDesktopState } from './useDesktopState'

const StateHarness = defineComponent({
  setup: useDesktopState,
  template: '<div />',
})

function receiverSnapshot(session: DesktopSnapshot['session']): DesktopSnapshot {
  return {
    session,
    outputDevices: [],
    selectedOutputId: null,
    pairing: { state: 'inactive', expiresInSeconds: null },
    metrics: {
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
    },
    previewAdapter: true,
  }
}

function deferred<T>() {
  let resolve: (value: T) => void
  let reject: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return {
    promise,
    resolve: (value: T) => resolve(value),
    reject: (reason?: unknown) => reject(reason),
  }
}

enableAutoUnmount(afterEach)

afterEach(() => {
  vi.resetAllMocks()
})

describe('useDesktopState', () => {
  it('accepts only one bridge command while a prior command is pending', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    const pendingSnapshot = deferred<DesktopSnapshot>()
    desktopBridgeMock.snapshot.mockImplementation(() => pendingSnapshot.promise)

    const first = wrapper.vm.refresh()
    const second = wrapper.vm.refresh()

    expect(desktopBridgeMock.snapshot).toHaveBeenCalledTimes(2)

    pendingSnapshot.resolve(receiverSnapshot('streaming'))
    await Promise.all([first, second])

    expect(wrapper.vm.snapshot?.session).toBe('streaming')
  })

  it('forwards a preview connection-loss simulation and projects its snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.simulateConnectionLost.mockResolvedValueOnce(receiverSnapshot('reconnecting'))

    await wrapper.vm.simulateConnectionLost()

    expect(desktopBridgeMock.simulateConnectionLost).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('reconnecting')
  })

  it('forwards a revoke command and projects the returned snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.revokePairing.mockResolvedValueOnce({
      ...receiverSnapshot('idle'),
      pairing: { state: 'revoked', expiresInSeconds: null },
    })

    await wrapper.vm.revokePairing()

    expect(desktopBridgeMock.revokePairing).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('idle')
    expect(wrapper.vm.snapshot?.pairing.state).toBe('revoked')
  })

  it('recovers from a rejected bridge command before accepting a later command', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    const pendingStart = deferred<DesktopSnapshot>()
    desktopBridgeMock.startStream.mockImplementationOnce(() => pendingStart.promise)

    const start = wrapper.vm.start()
    expect(wrapper.vm.busy).toBe(true)
    expect(wrapper.vm.error).toBe(null)

    pendingStart.reject(new Error('desktop control unavailable'))
    await start

    expect(wrapper.vm.busy).toBe(false)
    expect(wrapper.vm.error).toBe('The desktop control service did not accept this command.')
    expect(wrapper.vm.snapshot?.session).toBe('ready')

    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('streaming'))
    await wrapper.vm.refresh()

    expect(wrapper.vm.busy).toBe(false)
    expect(wrapper.vm.error).toBe(null)
    expect(wrapper.vm.snapshot?.session).toBe('streaming')
    expect(desktopBridgeMock.snapshot).toHaveBeenCalledTimes(2)
  })
  it('forwards a pairing-consumed simulation and projects its snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.simulatePairingConsumed.mockResolvedValueOnce(receiverSnapshot('ready'))

    await wrapper.vm.simulatePairingConsumed()

    expect(desktopBridgeMock.simulatePairingConsumed).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('ready')
  })

  it('forwards a preview metrics burst and projects its snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.simulateMetricsBurst.mockResolvedValueOnce(receiverSnapshot('streaming'))

    await wrapper.vm.simulateMetricsBurst()

    expect(desktopBridgeMock.simulateMetricsBurst).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('streaming')
  })

  it('surfaces a rejected bridge command as the control error banner', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.simulateCommandRejection.mockRejectedValueOnce(new Error('rejected'))

    await wrapper.vm.simulateCommandRejection()

    expect(desktopBridgeMock.simulateCommandRejection).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.error).toBe('The desktop control service did not accept this command.')
    expect(wrapper.vm.busy).toBe(false)
  })

  it('forwards a preview reset and projects the restored snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.resetPreview.mockResolvedValueOnce(receiverSnapshot('ready'))

    await wrapper.vm.resetPreview()

    expect(desktopBridgeMock.resetPreview).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('ready')
  })

})
