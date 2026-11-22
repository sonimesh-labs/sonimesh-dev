import { describe, expect, it, vi } from 'vitest'

import { TauriDesktopBridge } from './bridge'
import type { DesktopBridgeInvoke } from './bridge'
import type { DesktopSnapshot } from './types'

function snapshot(): DesktopSnapshot {
  return {
    session: 'ready',
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
    reconnectRemainingMs: null,
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
      targetMinFrames: 1,
      targetMaxFrames: 2,
      targetFrames: 1,
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
      playoutDelayMs: 20,
    },
    replies: [],
    diagnosticExports: [],
    previewAdapter: true,
  }
}

describe('TauriDesktopBridge', () => {
  it('maps production commands to low-rate IPC and marks snapshots as live', async () => {
    const calls: Array<[string, Record<string, unknown> | undefined]> = []
    const invoke: DesktopBridgeInvoke = async <T>(command: string, args?: Record<string, unknown>) => {
      calls.push([command, args])
      return snapshot() as T
    }
    const bridge = new TauriDesktopBridge(invoke)

    await bridge.snapshot()
    await bridge.startStream()
    await bridge.selectOutput('speakers')
    await bridge.setGain(0.5)
    await bridge.setMuted(true)
    await bridge.setJitterMode('stable')
    await bridge.exportDiagnostics()
    await bridge.createPairing()
    await bridge.expirePairing()
    await bridge.revokePairing()

    expect(calls).toEqual([
      ['desktop_snapshot', undefined],
      ['desktop_start_stream', undefined],
      ['desktop_select_output', { deviceId: 'speakers' }],
      ['desktop_set_gain', { gain: 0.5 }],
      ['desktop_set_muted', { muted: true }],
      ['desktop_set_jitter_mode', { mode: 'stable' }],
      ['desktop_export_diagnostics', undefined],
      ['desktop_create_pairing', undefined],
      ['desktop_expire_pairing', undefined],
      ['desktop_revoke_pairing', undefined],
    ])
    await expect(bridge.snapshot()).resolves.toMatchObject({ previewAdapter: false })
  })

  it('preserves IPC failures for the UI error path', async () => {
    const invoke = vi.fn().mockRejectedValue(new Error('receiver unavailable'))
    const bridge = new TauriDesktopBridge(invoke)

    await expect(bridge.startStream()).rejects.toThrow('receiver unavailable')
  })

  it('does not expose preview-only controls in the production adapter', async () => {
    const bridge = new TauriDesktopBridge(vi.fn())

    await expect(bridge.simulateMetricsBurst()).rejects.toThrow('preview adapter')
    await expect(bridge.resetPreview()).rejects.toThrow('preview adapter')
  })
})
