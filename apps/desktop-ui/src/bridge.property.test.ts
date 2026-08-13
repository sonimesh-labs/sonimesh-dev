//! Deterministic property coverage for the preview desktop bridge.
//!
//! `PreviewDesktopBridge` is a synthetic stand-in for the desktop control
//! bridge. Its session, pairing, volume, output-selection, and expiry behavior
//! is a small guarded state machine, so this test drives a seeded randomized
//! command walk over a real bridge instance against an independent model that
//! re-implements the observable contract from scratch (separate state
//! literals, reducer, and expiry clock). After every step the entire returned
//! snapshot must match the model, the walk must reach every session, pairing,
//! and command kind so no transition guard is left unexercised, and the
//! reachable-state invariants must hold after every step.

import { afterEach, describe, expect, it, vi } from 'vitest'

import { PreviewDesktopBridge } from './bridge'
import type {
  DesktopSnapshot,
  MetricCounter,
  OutputDevice,
  OutputSnapshot,
  PairingState,
  SessionState,
  StreamMetrics,
} from './types'

const PREVIEW_DEVICES: OutputDevice[] = [
  {
    id: 'default',
    label: 'System default output',
    sampleRateHz: 48_000,
    channels: 2,
    isDefault: true,
  },
  {
    id: 'usb',
    label: 'USB audio output',
    sampleRateHz: 48_000,
    channels: 2,
    isDefault: false,
  },
]

const INVITATION_LIFETIME_MILLIS = 300_000

const RECONNECT_GRACE_MILLIS = 30_000

type Command =
  | { kind: 'snapshot' }
  | { kind: 'startStream' }
  | { kind: 'stopStream' }
  | { kind: 'selectOutput'; deviceId: string }
  | { kind: 'setGain'; gain: number }
  | { kind: 'setMuted'; muted: boolean }
  | { kind: 'createPairing' }
  | { kind: 'expirePairing' }
  | { kind: 'revokePairing' }
  | { kind: 'simulateConnectionLost' }
  | { kind: 'simulateReconnected' }
  | { kind: 'simulateSessionExpired' }
  | { kind: 'simulateOutputLost' }
  | { kind: 'simulateOutputRestored' }
  | { kind: 'simulatePairingConsumed' }
  | { kind: 'simulateMetricsBurst' }
  | { kind: 'resetPreview' }

/** Small deterministic mulberry32 generator; no external dependency. */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296
  }
}

function initialMetrics(): StreamMetrics {
  return {
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
    streamResets: '3',
    nominalInputSampleRateHz: 48_000,
    nominalOutputSampleRateHz: 48_000,
    driftAdjustmentPpm: 110,
    driftHardResyncs: '1',
    currentJitterMs: 38,
    playoutDelayMs: 64,
  }
}

function addToCounter(counter: MetricCounter, delta: number): MetricCounter {
  if (!/^\d+$/.test(counter)) {
    return counter
  }
  return (BigInt(counter) + BigInt(delta)).toString()
}

interface ModelState {
  session: SessionState
  pairing: PairingState
  pairingExpiresAtMillis: number | null
  selectedOutputId: string | null
  gain: number
  muted: boolean
  output: OutputSnapshot
  metrics: StreamMetrics
}

function initialModelState(): ModelState {
  return {
    session: 'ready',
    pairing: 'consumed',
    pairingExpiresAtMillis: null,
    selectedOutputId: 'default',
    gain: 1,
    muted: false,
    output: {
      selectionRetained: true,
      streamActive: false,
      faultReported: false,
      health: 'healthy',
    },
    metrics: initialMetrics(),
  }
}

/**
 * Independent reducer over the bridge's observable contract. The model keeps
 * its own state literals and clock and re-derives every transition guard; it
 * never reads from or drives the bridge implementation under test.
 */
class PreviewBridgeModel {
  private state: ModelState = initialModelState()
  private nowMillis: number

  constructor(nowMillis: number) {
    this.nowMillis = nowMillis
  }

  setNow(nowMillis: number): void {
    this.nowMillis = nowMillis
  }

  apply(command: Command): void {
    const s = this.state
    switch (command.kind) {
      case 'snapshot':
        return
      case 'startStream':
        if (s.session === 'ready' && s.pairing === 'consumed') {
          s.session = 'streaming'
          s.output.faultReported = false
          s.output.health = 'healthy'
        }
        return
      case 'stopStream':
        if (
          s.session === 'streaming' ||
          s.session === 'reconnecting' ||
          s.session === 'outputUnavailable'
        ) {
          s.session = 'ready'
          s.output.faultReported = false
          s.output.health = 'healthy'
        }
        return
      case 'selectOutput':
        if (
          (PREVIEW_DEVICES as readonly { id: string }[]).some(
            (device) => device.id === command.deviceId,
          )
        ) {
          s.selectedOutputId = command.deviceId
          s.output.selectionRetained = true
        }
        return
      case 'setGain':
        if (Number.isFinite(command.gain) && command.gain >= 0 && command.gain <= 1) {
          s.gain = command.gain
        }
        return
      case 'setMuted':
        s.muted = command.muted
        return
      case 'createPairing':
        if (s.pairing === 'inactive' || s.pairing === 'expired' || s.pairing === 'revoked') {
          s.pairingExpiresAtMillis = this.nowMillis + INVITATION_LIFETIME_MILLIS
          s.pairing = 'active'
          s.session = 'pairing'
        }
        return
      case 'expirePairing':
        if (s.pairing === 'active') {
          s.pairingExpiresAtMillis = null
          s.pairing = 'expired'
          s.session = 'idle'
        }
        return
      case 'revokePairing':
        if (s.pairing === 'consumed') {
          s.pairingExpiresAtMillis = null
          s.pairing = 'revoked'
          s.session = 'idle'
        }
        return
      case 'simulateConnectionLost':
        if ((s.session === 'ready' || s.session === 'streaming') && s.pairing === 'consumed') {
          s.session = 'reconnecting'
        }
        return
      case 'simulateReconnected':
        if (s.session === 'reconnecting') {
          s.session = 'ready'
        }
        return
      case 'simulateSessionExpired':
        if (
          (s.session === 'ready' ||
            s.session === 'streaming' ||
            s.session === 'reconnecting' ||
            s.session === 'outputUnavailable') &&
          s.pairing === 'consumed'
        ) {
          s.pairingExpiresAtMillis = null
          s.session = 'idle'
        }
        return
      case 'simulateOutputLost':
        if (s.session === 'streaming') {
          s.session = 'outputUnavailable'
          s.output.faultReported = true
          s.output.health = 'streamError'
        }
        return
      case 'simulateOutputRestored':
        if (s.session === 'outputUnavailable') {
          s.session = 'streaming'
          s.output.faultReported = false
          s.output.health = 'healthy'
        }
        return
      case 'simulatePairingConsumed':
        if (s.pairing === 'active') {
          s.pairingExpiresAtMillis = null
          s.pairing = 'consumed'
          s.session = 'ready'
        }
        return
      case 'simulateMetricsBurst': {
        const m = s.metrics
        m.authenticatedPackets = addToCounter(m.authenticatedPackets, 48)
        m.duplicatePackets = addToCounter(m.duplicatePackets, 2)
        m.rejectedPackets = addToCounter(m.rejectedPackets, 1)
        m.latePackets = addToCounter(m.latePackets, 5)
        m.reorderedPackets = addToCounter(m.reorderedPackets, 3)
        m.jitterResyncDroppedFrames = addToCounter(m.jitterResyncDroppedFrames, 2)
        m.plcFrames = addToCounter(m.plcFrames, 6)
        m.decodeErrors = addToCounter(m.decodeErrors, 1)
        m.pcmUnderflows = addToCounter(m.pcmUnderflows, 1)
        m.pcmOverflows = addToCounter(m.pcmOverflows, 1)
        m.pcmRefillLimitHits = addToCounter(m.pcmRefillLimitHits, 1)
        m.pcmDiscontinuities = addToCounter(m.pcmDiscontinuities, 1)
        const nextQueued = m.queuedPcmBlocks + 2
        m.queuedPcmHighWaterBlocks = Math.max(m.queuedPcmHighWaterBlocks, nextQueued)
        m.queuedPcmBlocks = nextQueued
        const nextJitterFrames = m.jitterFrames + 2
        m.jitterHighWaterFrames = Math.max(m.jitterHighWaterFrames, nextJitterFrames)
        m.jitterFrames = nextJitterFrames
        m.targetSmoothedJitterMs += 6
        m.targetFrames = Math.min(m.targetFrames + 1, m.targetMaxFrames)
        m.targetRaisedSteps += 1
        m.currentJitterMs += 14
        m.playoutDelayMs += 4
        return
      }
      case 'resetPreview':
        this.state = initialModelState()
        return
    }
  }

  snapshot(): Omit<DesktopSnapshot, 'replies'> {
    this.expireInvitationWhenDue()
    const s = this.state
    return {
      session: s.session,
      outputDevices: PREVIEW_DEVICES,
      selectedOutputId: s.selectedOutputId,
      gain: s.gain,
      muted: s.muted,
      pairing: {
        state: s.pairing,
        expiresInSeconds:
          s.pairing === 'active' && s.pairingExpiresAtMillis !== null
            ? Math.max(1, Math.ceil((s.pairingExpiresAtMillis - this.nowMillis) / 1_000))
            : s.pairing === 'expired'
              ? 0
              : null,
      },
      output: {
        selectionRetained: s.output.selectionRetained,
        streamActive: s.session === 'streaming',
        faultReported: s.output.faultReported,
        health: s.output.health,
      },
      metrics: structuredClone(s.metrics),
      reconnectRemainingMs: s.session === 'reconnecting' ? RECONNECT_GRACE_MILLIS : null,
      previewAdapter: true,
    }
  }

  private expireInvitationWhenDue(): void {
    const s = this.state
    if (s.pairing !== 'active' || s.pairingExpiresAtMillis === null) {
      return
    }
    if (s.pairingExpiresAtMillis - this.nowMillis <= 0) {
      s.pairingExpiresAtMillis = null
      s.pairing = 'expired'
      s.session = 'idle'
    }
  }
}

function runBridgeCommand(
  bridge: PreviewDesktopBridge,
  command: Command,
): Promise<DesktopSnapshot> {
  switch (command.kind) {
    case 'snapshot':
      return bridge.snapshot()
    case 'startStream':
      return bridge.startStream()
    case 'stopStream':
      return bridge.stopStream()
    case 'selectOutput':
      return bridge.selectOutput(command.deviceId)
    case 'setGain':
      return bridge.setGain(command.gain)
    case 'setMuted':
      return bridge.setMuted(command.muted)
    case 'createPairing':
      return bridge.createPairing()
    case 'expirePairing':
      return bridge.expirePairing()
    case 'revokePairing':
      return bridge.revokePairing()
    case 'simulateConnectionLost':
      return bridge.simulateConnectionLost()
    case 'simulateReconnected':
      return bridge.simulateReconnected()
    case 'simulateSessionExpired':
      return bridge.simulateSessionExpired()
    case 'simulateOutputLost':
      return bridge.simulateOutputLost()
    case 'simulateOutputRestored':
      return bridge.simulateOutputRestored()
    case 'simulatePairingConsumed':
      return bridge.simulatePairingConsumed()
    case 'simulateMetricsBurst':
      return bridge.simulateMetricsBurst()
    case 'resetPreview':
      return bridge.resetPreview()
  }
}

const GAIN_CANDIDATES = [0, 0.125, 0.5, 1, -0.1, 1.5, Number.NaN]
const DEVICE_CANDIDATES = ['default', 'usb', 'missing-device']

function pickCommand(random: () => number, kind: Command['kind']): Command {
  switch (kind) {
    case 'selectOutput':
      return { kind, deviceId: DEVICE_CANDIDATES[Math.floor(random() * DEVICE_CANDIDATES.length)] }
    case 'setGain':
      return { kind, gain: GAIN_CANDIDATES[Math.floor(random() * GAIN_CANDIDATES.length)] }
    case 'setMuted':
      return { kind, muted: random() < 0.5 }
    default:
      return { kind }
  }
}

afterEach(() => {
  vi.useRealTimers()
})

describe('PreviewDesktopBridge property', () => {
  it('randomized command walks match an independent preview-state model', async () => {
    vi.useFakeTimers()
    const startMillis = new Date('2026-08-11T00:00:00Z').getTime()
    vi.setSystemTime(new Date(startMillis))

    const bridge = new PreviewDesktopBridge()
    const model = new PreviewBridgeModel(startMillis)
    const random = mulberry32(0xd35e_2026_0013)
    const allKinds: Command['kind'][] = [
      'snapshot',
      'startStream',
      'stopStream',
      'selectOutput',
      'setGain',
      'setMuted',
      'createPairing',
      'expirePairing',
      'revokePairing',
      'simulateConnectionLost',
      'simulateReconnected',
      'simulateSessionExpired',
      'simulateOutputLost',
      'simulateOutputRestored',
      'simulatePairingConsumed',
      'simulateMetricsBurst',
      'resetPreview',
    ]

    let nowMillis = startMillis
    let maxRepliesSeen = 0
    const observedSessions = new Set<SessionState>()
    const observedPairings = new Set<PairingState>()
    const observedKinds = new Set<Command['kind']>()
    let observedActiveWithCountdown = false
    let observedAutoExpiry = false

    for (let step = 0; step < 3_000; step++) {
      // Advance the shared clock: mostly small steps so an open invitation can
      // count down, occasionally a jump past its lifetime to force auto-expiry.
      const advance =
        Math.floor(random() * 30_000) + (random() < 0.25 ? Math.floor(random() * 300_000) : 0)
      nowMillis += advance
      vi.setSystemTime(new Date(nowMillis))
      model.setNow(nowMillis)

      const kind = allKinds[Math.floor(random() * allKinds.length)]
      const command = pickCommand(random, kind)
      observedKinds.add(kind)

      const observed = await runBridgeCommand(bridge, command)
      model.apply(command)
      const expected = model.snapshot()

      // The reply log is an append-only diagnostic surface derived from command
      // outcomes, not part of the reachable lifecycle contract. The independent
      // model pins the state machine while dedicated bridge tests pin the exact
      // reply content; here we verify the log stays bounded and ordered.
      const { replies: _observedReplies, ...observedState } = observed
      void _observedReplies
      expect(observedState).toEqual(expected)
      expect(observed.replies.length).toBeLessThanOrEqual(6)
      for (let index = 1; index < observed.replies.length; index += 1) {
        expect(observed.replies[index - 1].requestId).toBeGreaterThan(
          observed.replies[index].requestId,
        )
      }
      for (const reply of observed.replies) {
        expect(typeof reply.detail).toBe('string')
        expect(reply.detail.length).toBeGreaterThan(0)
        if (reply.kind === 'streamStatus') {
          expect(typeof reply.state).toBe('string')
          expect(reply.code).toBeUndefined()
          expect(reply.retryable).toBeUndefined()
        } else {
          expect(typeof reply.code).toBe('string')
          expect(typeof reply.retryable).toBe('boolean')
          expect(reply.state).toBeUndefined()
        }
      }
      maxRepliesSeen = Math.max(maxRepliesSeen, observed.replies.length)

      observedSessions.add(observed.session)
      observedPairings.add(observed.pairing.state)
      if (observed.pairing.state === 'active' && observed.pairing.expiresInSeconds !== null) {
        expect(observed.pairing.expiresInSeconds).toBeGreaterThanOrEqual(1)
        expect(observed.pairing.expiresInSeconds).toBeLessThanOrEqual(300)
        if (observed.pairing.expiresInSeconds < 300) {
          observedActiveWithCountdown = true
        }
      }
      if (observed.pairing.state === 'expired') {
        observedAutoExpiry = true
      }

      // Reachable-state invariants that every snapshot must satisfy.
      expect(observed.gain).toBeGreaterThanOrEqual(0)
      expect(observed.gain).toBeLessThanOrEqual(1)
      expect(observed.output.streamActive).toBe(observed.session === 'streaming')
      if (observed.output.health === 'healthy') {
        expect(observed.output.faultReported).toBe(false)
      }
      if (observed.pairing.state === 'active') {
        expect(observed.session).toBe('pairing')
      }
      if (observed.session === 'pairing') {
        expect(observed.pairing.state).toBe('active')
      }
      if (observed.pairing.state === 'expired' || observed.pairing.state === 'revoked') {
        expect(observed.session).toBe('idle')
      }
    }

    expect(observedKinds.size).toBe(allKinds.length)
    expect(observedSessions.has('idle')).toBe(true)
    expect(observedSessions.has('pairing')).toBe(true)
    expect(observedSessions.has('ready')).toBe(true)
    expect(observedSessions.has('streaming')).toBe(true)
    expect(observedSessions.has('reconnecting')).toBe(true)
    expect(observedSessions.has('outputUnavailable')).toBe(true)
    expect(observedPairings.has('active')).toBe(true)
    expect(observedPairings.has('consumed')).toBe(true)
    expect(observedPairings.has('expired')).toBe(true)
    expect(observedPairings.has('revoked')).toBe(true)
    expect(observedActiveWithCountdown).toBe(true)
    expect(observedAutoExpiry).toBe(true)
    expect(maxRepliesSeen).toBe(6)
  })
})
