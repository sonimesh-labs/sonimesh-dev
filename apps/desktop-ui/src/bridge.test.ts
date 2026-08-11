import { afterEach, describe, expect, it, vi } from 'vitest'

import { PreviewDesktopBridge } from './bridge'

afterEach(() => {
  vi.useRealTimers()
})

function fixedTimeBridge(): PreviewDesktopBridge {
  vi.useFakeTimers()
  vi.setSystemTime(new Date('2026-08-11T00:00:00Z'))
  return new PreviewDesktopBridge()
}

describe('PreviewDesktopBridge', () => {
  it('starts with synthetic consumed trust so session controls are usable', async () => {
    const snapshot = await new PreviewDesktopBridge().snapshot()

    expect(snapshot.session).toBe('ready')
    expect(snapshot.pairing).toEqual({ state: 'consumed', expiresInSeconds: null })
    expect(snapshot.previewAdapter).toBe(true)
  })

  it('toggles a streaming session under synthetic trust', async () => {
    const bridge = new PreviewDesktopBridge()

    await expect(bridge.startStream()).resolves.toMatchObject({ session: 'streaming' })
    await expect(bridge.stopStream()).resolves.toMatchObject({ session: 'ready' })
  })

  it('does not open a preview invitation while synthetic trust is held', async () => {
    const bridge = new PreviewDesktopBridge()

    const snapshot = await bridge.createPairing()

    expect(snapshot.session).toBe('ready')
    expect(snapshot.pairing.state).toBe('consumed')
  })

  it('revokes synthetic trust, then opens a fresh preview invitation', async () => {
    const bridge = fixedTimeBridge()

    const revoked = await bridge.revokePairing()
    expect(revoked.session).toBe('idle')
    expect(revoked.pairing).toEqual({ state: 'revoked', expiresInSeconds: null })

    const active = await bridge.createPairing()
    expect(active.session).toBe('pairing')
    expect(active.pairing.state).toBe('active')
    expect(active.pairing.expiresInSeconds).toBe(300)
  })

  it('retires an active preview invitation and allows a fresh one', async () => {
    const bridge = fixedTimeBridge()
    await bridge.revokePairing()
    await bridge.createPairing()

    const expired = await bridge.expirePairing()
    expect(expired.session).toBe('idle')
    expect(expired.pairing).toEqual({ state: 'expired', expiresInSeconds: 0 })

    const active = await bridge.createPairing()
    expect(active.session).toBe('pairing')
    expect(active.pairing.state).toBe('active')
  })

  it('auto-expires a preview invitation on the next snapshot after its lifetime', async () => {
    const bridge = fixedTimeBridge()
    await bridge.revokePairing()
    const active = await bridge.createPairing()
    expect(active.pairing.expiresInSeconds).toBe(300)

    await vi.advanceTimersByTimeAsync(299_000)
    expect((await bridge.snapshot()).pairing.expiresInSeconds).toBe(1)

    await vi.advanceTimersByTimeAsync(1_000)
    const expired = await bridge.snapshot()
    expect(expired.session).toBe('idle')
    expect(expired.pairing).toEqual({ state: 'expired', expiresInSeconds: 0 })
  })

  it('revoking synthetic trust returns a streaming preview to idle', async () => {
    const bridge = new PreviewDesktopBridge()
    await bridge.startStream()

    const snapshot = await bridge.revokePairing()

    expect(snapshot.session).toBe('idle')
    expect(snapshot.pairing.state).toBe('revoked')
  })

  it('simulates connection loss from a trusted session and reconnects to ready', async () => {
    const bridge = new PreviewDesktopBridge()
    await bridge.startStream()

    const reconnecting = await bridge.simulateConnectionLost()
    expect(reconnecting.session).toBe('reconnecting')
    expect(reconnecting.pairing.state).toBe('consumed')

    const reconnected = await bridge.simulateReconnected()
    expect(reconnected.session).toBe('ready')
  })

  it('does not simulate connection loss without a trusted session', async () => {
    const bridge = new PreviewDesktopBridge()
    await bridge.revokePairing()

    const snapshot = await bridge.simulateConnectionLost()
    expect(snapshot.session).toBe('idle')
  })

  it('simulates output loss from streaming and restores it', async () => {
    const bridge = new PreviewDesktopBridge()
    await bridge.startStream()

    const unavailable = await bridge.simulateOutputLost()
    expect(unavailable.session).toBe('outputUnavailable')

    const restored = await bridge.simulateOutputRestored()
    expect(restored.session).toBe('streaming')
  })

  it('does not simulate output loss outside streaming', async () => {
    const bridge = new PreviewDesktopBridge()

    const snapshot = await bridge.simulateOutputLost()
    expect(snapshot.session).toBe('ready')
  })

  it('does not replace an active preview invitation', async () => {
    const bridge = fixedTimeBridge()
    await bridge.revokePairing()
    await bridge.createPairing()

    const again = await bridge.createPairing()

    expect(again.pairing.state).toBe('active')
    expect(again.pairing.expiresInSeconds).toBe(300)
  })
  it('simulates the sender consuming an open preview invitation', async () => {
    const bridge = fixedTimeBridge()
    await bridge.revokePairing()
    await bridge.createPairing()

    const consumed = await bridge.simulatePairingConsumed()

    expect(consumed.session).toBe('ready')
    expect(consumed.pairing).toEqual({ state: 'consumed', expiresInSeconds: null })
  })

  it('does not consume an invitation that is not active', async () => {
    const bridge = new PreviewDesktopBridge()

    const snapshot = await bridge.simulatePairingConsumed()

    expect(snapshot.session).toBe('ready')
    expect(snapshot.pairing.state).toBe('consumed')
  })

  it('simulates a burst of transport and recovery activity and accumulates on repeat', async () => {
    const bridge = new PreviewDesktopBridge()

    const before = await bridge.snapshot()
    const burst = await bridge.simulateMetricsBurst()
    const again = await bridge.simulateMetricsBurst()

    expect(burst.metrics.authenticatedPackets).toBe('12892')
    expect(burst.metrics.duplicatePackets).toBe('8')
    expect(burst.metrics.plcFrames).toBe('13')
    expect(burst.metrics.currentJitterMs).toBe(52)
    expect(burst.metrics.playoutDelayMs).toBe(68)
    expect(burst.metrics.queuedPcmBlocks).toBe(5)
    expect(burst.metrics.queuedPcmHighWaterBlocks).toBe(5)

    expect(again.metrics.authenticatedPackets).toBe('12940')
    expect(again.metrics.currentJitterMs).toBe(66)

    expect(burst.metrics.streamResets).toBe(before.metrics.streamResets)
    expect(burst.metrics.driftHardResyncs).toBe(before.metrics.driftHardResyncs)
  })

  it('keeps stable counters unchanged across a metrics burst', async () => {
    const bridge = new PreviewDesktopBridge()

    const burst = await bridge.simulateMetricsBurst()

    expect(burst.metrics.streamResets).toBe('3')
    expect(burst.metrics.driftHardResyncs).toBe('1')
    expect(burst.metrics.nominalInputSampleRateHz).toBe(48_000)
    expect(burst.metrics.nominalOutputSampleRateHz).toBe(48_000)
  })

  it('rejects like a bridge command so the error path is exercisable', async () => {
    const bridge = new PreviewDesktopBridge()

    await expect(bridge.simulateCommandRejection()).rejects.toThrow('preview command rejection')
  })

  it('resets the preview to its initial synthetic state after simulations', async () => {
    const bridge = new PreviewDesktopBridge()
    await bridge.startStream()
    await bridge.simulateMetricsBurst()
    await bridge.revokePairing()
    await bridge.createPairing()
    expect((await bridge.snapshot()).session).toBe('pairing')

    const reset = await bridge.resetPreview()

    expect(reset.session).toBe('ready')
    expect(reset.pairing).toEqual({ state: 'consumed', expiresInSeconds: null })
    expect(reset.metrics.authenticatedPackets).toBe('12844')
    expect(reset.metrics.currentJitterMs).toBe(38)
    expect(reset.metrics.queuedPcmBlocks).toBe(3)
    expect(reset.previewAdapter).toBe(true)
  })
})
