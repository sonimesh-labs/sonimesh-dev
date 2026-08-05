import { defineComponent } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'

import type { DesktopSnapshot } from './types'

const desktopBridgeMock = vi.hoisted(() => ({
  createPairing: vi.fn(),
  expirePairing: vi.fn(),
  selectOutput: vi.fn(),
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
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise
  })
  return { promise, resolve: (value: T) => resolve(value) }
}

enableAutoUnmount(afterEach)

afterEach(() => {
  vi.clearAllMocks()
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
})
