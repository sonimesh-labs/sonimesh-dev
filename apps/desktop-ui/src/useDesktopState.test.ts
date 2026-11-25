import { defineComponent } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'

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
  setJitterMode: vi.fn(),
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
    gain: 1,
    muted: false,
    output: {
      selectionRetained: false,
      streamActive: false,
      faultReported: false,
      health: 'healthy',
    },
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
    },
    replies: [],
    diagnosticExports: [],
    reconnectRemainingMs: null,
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
  it('refreshes real output devices on focus and retains the updated selection', async () => {
    const initial = { ...receiverSnapshot('idle'), previewAdapter: false, selectedOutputId: 'old' }
    const updated = { ...initial, selectedOutputId: 'new' }
    desktopBridgeMock.snapshot.mockResolvedValue(initial)
    desktopBridgeMock.refreshOutputDevices.mockResolvedValue(updated)
    const wrapper = mount(StateHarness)
    await flushPromises()

    window.dispatchEvent(new Event('focus'))
    window.dispatchEvent(new Event('focus'))
    await new Promise((resolve) => setTimeout(resolve, 10))
    await flushPromises()
    expect(desktopBridgeMock.refreshOutputDevices).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.selectedOutputId).toBe('new')
    wrapper.unmount()
    window.dispatchEvent(new Event('focus'))
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(desktopBridgeMock.refreshOutputDevices).toHaveBeenCalledTimes(1)
  })

  it('defers focus refresh until an ongoing command finishes', async () => {
    const initial = { ...receiverSnapshot('idle'), previewAdapter: false }
    desktopBridgeMock.snapshot.mockResolvedValue(initial)
    desktopBridgeMock.refreshOutputDevices.mockResolvedValue(initial)
    const wrapper = mount(StateHarness)
    await flushPromises()
    const pending = deferred<DesktopSnapshot>()
    desktopBridgeMock.selectOutput.mockReturnValueOnce(pending.promise)
    const selection = wrapper.vm.selectOutput('device-b')
    window.dispatchEvent(new Event('focus'))
    window.dispatchEvent(new Event('focus'))
    expect(desktopBridgeMock.refreshOutputDevices).not.toHaveBeenCalled()
    pending.resolve(initial)
    await selection
    await new Promise((resolve) => setTimeout(resolve, 10))
    await flushPromises()
    expect(desktopBridgeMock.refreshOutputDevices).toHaveBeenCalledTimes(1)
  })

  it('does not refresh synthetic devices when the preview gains focus', async () => {
    desktopBridgeMock.snapshot.mockResolvedValue(receiverSnapshot('ready'))
    mount(StateHarness)
    await flushPromises()
    window.dispatchEvent(new Event('focus'))
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(desktopBridgeMock.refreshOutputDevices).not.toHaveBeenCalled()
  })

  it('keeps a startup error visible while the next snapshot is pending', async () => {
    desktopBridgeMock.snapshot.mockRejectedValueOnce(new Error('enumeration failed'))
    const wrapper = mount(StateHarness)
    await flushPromises()
    expect(wrapper.vm.error).not.toBeNull()

    const pending = deferred<DesktopSnapshot>()
    desktopBridgeMock.snapshot.mockReturnValueOnce(pending.promise)
    const refresh = wrapper.vm.refresh()
    expect(wrapper.vm.error).not.toBeNull()
    pending.resolve(receiverSnapshot('ready'))
    await refresh
    expect(wrapper.vm.error).toBeNull()
  })
  it('polls the bridge for low-rate state and stops polling after unmount', async () => {
    vi.useFakeTimers()
    desktopBridgeMock.snapshot.mockResolvedValue(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    expect(desktopBridgeMock.snapshot).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(1_000)
    await flushPromises()
    expect(desktopBridgeMock.snapshot).toHaveBeenCalledTimes(2)

    wrapper.unmount()
    vi.advanceTimersByTime(2_000)
    await flushPromises()
    expect(desktopBridgeMock.snapshot).toHaveBeenCalledTimes(2)
    vi.useRealTimers()
  })

  it('keeps the last snapshot when a background poll has no response', async () => {
    vi.useFakeTimers()
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.snapshot.mockResolvedValueOnce(undefined)
    vi.advanceTimersByTime(1_000)
    await flushPromises()

    expect(wrapper.vm.snapshot?.session).toBe('ready')
    wrapper.unmount()
    vi.useRealTimers()
  })

  it('keeps user commands available during a pending snapshot and ignores its stale result', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    const pendingSnapshot = deferred<DesktopSnapshot>()
    desktopBridgeMock.snapshot.mockReturnValueOnce(pendingSnapshot.promise)
    desktopBridgeMock.selectOutput.mockResolvedValueOnce(receiverSnapshot('streaming'))

    const refresh = wrapper.vm.refresh()
    expect(wrapper.vm.refreshing).toBe(true)
    expect(wrapper.vm.busy).toBe(false)

    await wrapper.vm.selectOutput('device-b')
    expect(desktopBridgeMock.selectOutput).toHaveBeenCalledWith('device-b')
    expect(wrapper.vm.snapshot?.session).toBe('streaming')

    pendingSnapshot.resolve(receiverSnapshot('idle'))
    await refresh

    expect(wrapper.vm.snapshot?.session).toBe('streaming')
    expect(wrapper.vm.refreshing).toBe(false)
  })

  it('refreshes the output inventory through a serialized user command', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    const pendingRefresh = deferred<DesktopSnapshot>()
    desktopBridgeMock.refreshOutputDevices.mockReturnValueOnce(pendingRefresh.promise)
    const refresh = wrapper.vm.refreshOutputDevices()

    expect(wrapper.vm.busy).toBe(true)
    expect(wrapper.vm.refreshingDevices).toBe(true)
    pendingRefresh.resolve({ ...receiverSnapshot('ready'), selectedOutputId: 'device-b' })
    await refresh

    expect(wrapper.vm.busy).toBe(false)
    expect(wrapper.vm.refreshingDevices).toBe(false)
    expect(wrapper.vm.snapshot?.selectedOutputId).toBe('device-b')
  })

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

  it('forwards a preview diagnostic export and projects its snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.exportDiagnostics.mockResolvedValueOnce({
      ...receiverSnapshot('ready'),
      diagnosticExports: [{ sequence: '0', bytes: '16384' }],
    })

    await wrapper.vm.exportDiagnostics()

    expect(desktopBridgeMock.exportDiagnostics).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.diagnosticExports).toEqual([{ sequence: '0', bytes: '16384' }])
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

  it('forwards a preview session-expiry simulation and projects its snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.simulateSessionExpired.mockResolvedValueOnce(receiverSnapshot('idle'))

    await wrapper.vm.simulateSessionExpired()

    expect(desktopBridgeMock.simulateSessionExpired).toHaveBeenCalledTimes(1)
    expect(wrapper.vm.snapshot?.session).toBe('idle')
  })

  it('forwards gain and mute commands and projects the returned snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.setGain.mockResolvedValueOnce({ ...receiverSnapshot('ready'), gain: 0.5 })
    desktopBridgeMock.setMuted.mockResolvedValueOnce({ ...receiverSnapshot('ready'), muted: true })

    await wrapper.vm.setGain(0.5)

    expect(desktopBridgeMock.setGain).toHaveBeenCalledWith(0.5)
    expect(wrapper.vm.snapshot?.gain).toBe(0.5)

    await wrapper.vm.setMuted(true)

    expect(desktopBridgeMock.setMuted).toHaveBeenCalledWith(true)
    expect(wrapper.vm.snapshot?.muted).toBe(true)
  })

  it('reports successful completion of the selected-output test tone', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    desktopBridgeMock.testOutput.mockResolvedValueOnce(undefined)
    const wrapper = mount(StateHarness)
    await flushPromises()

    await wrapper.vm.testOutput()

    expect(desktopBridgeMock.testOutput).toHaveBeenCalledOnce()
    expect(wrapper.vm.outputTestStatus).toBe('Test tone completed on the selected output.')
    expect(wrapper.vm.error).toBe(null)

    desktopBridgeMock.refreshOutputDevices.mockResolvedValueOnce(receiverSnapshot('ready'))
    await wrapper.vm.refreshOutputDevices()
    expect(wrapper.vm.outputTestStatus).toBe('Test tone completed on the selected output.')

    await wrapper.vm.testOutput()
    desktopBridgeMock.refreshOutputDevices.mockResolvedValueOnce({
      ...receiverSnapshot('ready'),
      selectedOutputId: 'fallback-device',
    })
    await wrapper.vm.refreshOutputDevices()
    expect(wrapper.vm.outputTestStatus).toBe(null)

    desktopBridgeMock.selectOutput.mockResolvedValueOnce(receiverSnapshot('ready'))
    await wrapper.vm.selectOutput('other-output')
    expect(wrapper.vm.outputTestStatus).toBe(null)
  })

  it('reports output selection failures and releases the command state', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    desktopBridgeMock.selectOutput.mockRejectedValueOnce(new Error('device unavailable'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    await wrapper.vm.selectOutput('missing-device')

    expect(wrapper.vm.error).toBe('Could not select this output device. Refresh the device list and try again.')
    expect(wrapper.vm.busy).toBe(false)
  })

  it('reports output inventory refresh failures and releases both loading states', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    desktopBridgeMock.refreshOutputDevices.mockRejectedValueOnce(new Error('enumeration failed'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    await wrapper.vm.refreshOutputDevices()

    expect(wrapper.vm.error).toBe('Could not refresh output devices. Check the audio service and try again.')
    expect(wrapper.vm.busy).toBe(false)
    expect(wrapper.vm.refreshingDevices).toBe(false)
  })

  it('keeps a failed focus refresh visible across cached snapshots until inventory is refreshed', async () => {
    const initial = { ...receiverSnapshot('idle'), previewAdapter: false, selectedOutputId: 'old' }
    const updated = { ...initial, selectedOutputId: 'new' }
    desktopBridgeMock.snapshot.mockResolvedValue(initial)
    desktopBridgeMock.refreshOutputDevices.mockRejectedValueOnce(new Error('enumeration failed'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    window.dispatchEvent(new Event('focus'))
    await new Promise((resolve) => setTimeout(resolve, 10))
    await flushPromises()
    const warning = 'Could not refresh output devices. Check the audio service and try again.'
    expect(wrapper.vm.error).toBe(warning)
    expect(wrapper.vm.snapshot?.selectedOutputId).toBe('old')

    await wrapper.vm.refresh()
    expect(wrapper.vm.error).toBe(warning)
    desktopBridgeMock.refreshOutputDevices.mockResolvedValueOnce(updated)
    await wrapper.vm.refreshOutputDevices()
    expect(wrapper.vm.error).toBeNull()
    expect(wrapper.vm.snapshot?.selectedOutputId).toBe('new')
  })

  it('clears an inventory warning when selecting an output successfully re-enumerates devices', async () => {
    const initial = receiverSnapshot('ready')
    desktopBridgeMock.snapshot.mockResolvedValue(initial)
    desktopBridgeMock.refreshOutputDevices.mockRejectedValueOnce(new Error('enumeration failed'))
    desktopBridgeMock.selectOutput.mockResolvedValueOnce(initial)
    const wrapper = mount(StateHarness)
    await flushPromises()
    await wrapper.vm.refreshOutputDevices()
    expect(wrapper.vm.error).not.toBeNull()
    await wrapper.vm.selectOutput('device-b')
    expect(wrapper.vm.error).toBeNull()
  })

  it('reports test-tone failures without leaving a success result', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    desktopBridgeMock.testOutput.mockRejectedValueOnce(new Error('output unavailable'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    await wrapper.vm.testOutput()

    expect(wrapper.vm.error).toBe('Could not play a test tone on the selected output. Check the device and try again.')
    expect(wrapper.vm.outputTestStatus).toBe(null)
    expect(wrapper.vm.busy).toBe(false)
  })

  it('forwards the jitter mode command and projects the returned snapshot', async () => {
    desktopBridgeMock.snapshot.mockResolvedValueOnce(receiverSnapshot('ready'))
    const wrapper = mount(StateHarness)
    await flushPromises()

    desktopBridgeMock.setJitterMode.mockResolvedValueOnce({
      ...receiverSnapshot('ready'),
      metrics: { ...receiverSnapshot('ready').metrics, targetMinFrames: 4, targetMaxFrames: 6, targetFrames: 5 },
    })

    await wrapper.vm.setJitterMode('stable')

    expect(desktopBridgeMock.setJitterMode).toHaveBeenCalledWith('stable')
    expect(wrapper.vm.snapshot?.metrics.targetFrames).toBe(5)
  })

})
