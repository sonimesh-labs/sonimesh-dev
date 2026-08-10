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

  it('does not replace an active preview invitation', async () => {
    const bridge = fixedTimeBridge()
    await bridge.revokePairing()
    await bridge.createPairing()

    const again = await bridge.createPairing()

    expect(again.pairing.state).toBe('active')
    expect(again.pairing.expiresInSeconds).toBe(300)
  })
})
