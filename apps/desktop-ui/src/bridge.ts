import type { DesktopSnapshot, MetricCounter, OutputDevice, SessionState } from './types'

export interface DesktopBridge {
  snapshot(): Promise<DesktopSnapshot>
  startStream(): Promise<DesktopSnapshot>
  stopStream(): Promise<DesktopSnapshot>
  selectOutput(deviceId: string): Promise<DesktopSnapshot>
  /** Publishes a bounded linear gain target; out-of-range or non-finite values are ignored. */
  setGain(gain: number): Promise<DesktopSnapshot>
  /** Publishes the per-stream mute flag. */
  setMuted(muted: boolean): Promise<DesktopSnapshot>
  createPairing(): Promise<DesktopSnapshot>
  expirePairing(): Promise<DesktopSnapshot>
  revokePairing(): Promise<DesktopSnapshot>
  /** Preview-only: simulates losing the control connection from a trusted session. */
  simulateConnectionLost(): Promise<DesktopSnapshot>
  /** Preview-only: simulates a reconnected control connection returning to ready. */
  simulateReconnected(): Promise<DesktopSnapshot>
  /** Preview-only: simulates the trusted session expiring (trust or reconnect grace ends). */
  simulateSessionExpired(): Promise<DesktopSnapshot>
  /** Preview-only: simulates the active output device disappearing. */
  simulateOutputLost(): Promise<DesktopSnapshot>
  /** Preview-only: simulates a recreated output device resuming streaming. */
  simulateOutputRestored(): Promise<DesktopSnapshot>
  /** Preview-only: simulates the sender consuming the open pairing invitation. */
  simulatePairingConsumed(): Promise<DesktopSnapshot>
  /** Preview-only: simulates a burst of transport and recovery activity so counters visibly move. */
  simulateMetricsBurst(): Promise<DesktopSnapshot>
  /** Preview-only: rejects like a real bridge command so the error banner is exercisable. */
  simulateCommandRejection(): Promise<DesktopSnapshot>
  /** Preview-only: restores the initial synthetic state after preview simulations. */
  resetPreview(): Promise<DesktopSnapshot>
}

const previewOutput: OutputDevice[] = [
  { id: 'default', label: 'System default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
  { id: 'usb', label: 'USB audio output', sampleRateHz: 48_000, channels: 2, isDefault: false },
]

const INITIAL_PREVIEW_SNAPSHOT: DesktopSnapshot = {
  session: 'ready',
  outputDevices: previewOutput,
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

export class PreviewDesktopBridge implements DesktopBridge {
  private pairingExpiresAtMillis: number | null = null

  private snapshotState: DesktopSnapshot = structuredClone(INITIAL_PREVIEW_SNAPSHOT)

  async snapshot(): Promise<DesktopSnapshot> {
    return this.copy()
  }

  async startStream(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'ready' && this.snapshotState.pairing.state === 'consumed') {
      this.setSession('streaming')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
    }
    return this.copy()
  }

  async stopStream(): Promise<DesktopSnapshot> {
    if (['streaming', 'reconnecting', 'outputUnavailable'].includes(this.snapshotState.session)) {
      this.setSession('ready')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
    }
    return this.copy()
  }

  async selectOutput(deviceId: string): Promise<DesktopSnapshot> {
    if (this.snapshotState.outputDevices.some((device) => device.id === deviceId)) {
      this.snapshotState.selectedOutputId = deviceId
      this.snapshotState.output.selectionRetained = true
    }
    return this.copy()
  }

  async setGain(gain: number): Promise<DesktopSnapshot> {
    if (Number.isFinite(gain) && gain >= 0 && gain <= 1) {
      this.snapshotState.gain = gain
    }
    return this.copy()
  }

  async setMuted(muted: boolean): Promise<DesktopSnapshot> {
    this.snapshotState.muted = muted
    return this.copy()
  }

  async createPairing(): Promise<DesktopSnapshot> {
    if (['inactive', 'expired', 'revoked'].includes(this.snapshotState.pairing.state)) {
      this.pairingExpiresAtMillis = Date.now() + PAIRING_INVITATION_LIFETIME_MILLIS
      this.snapshotState.pairing = { state: 'active', expiresInSeconds: PAIRING_INVITATION_LIFETIME_SECONDS }
      this.setSession('pairing')
    }
    return this.copy()
  }

  async expirePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.setSession('idle')
    }
    return this.copy()
  }

  async revokePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'consumed') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'revoked', expiresInSeconds: null }
      this.setSession('idle')
    }
    return this.copy()
  }

  async simulateConnectionLost(): Promise<DesktopSnapshot> {
    if (
      ['ready', 'streaming'].includes(this.snapshotState.session) &&
      this.snapshotState.pairing.state === 'consumed'
    ) {
      this.setSession('reconnecting')
    }
    return this.copy()
  }

  async simulateReconnected(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'reconnecting') {
      this.setSession('ready')
    }
    return this.copy()
  }

  async simulateSessionExpired(): Promise<DesktopSnapshot> {
    if (
      ['ready', 'streaming', 'reconnecting', 'outputUnavailable'].includes(this.snapshotState.session) &&
      this.snapshotState.pairing.state === 'consumed'
    ) {
      // The session owner retires the trusted session on expiry while the
      // consumed pairing record stays retained for audit; revoke clears it.
      this.pairingExpiresAtMillis = null
      this.setSession('idle')
    }
    return this.copy()
  }

  async simulateOutputLost(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'streaming') {
      this.setSession('outputUnavailable')
      this.snapshotState.output.faultReported = true
      this.snapshotState.output.health = 'streamError'
    }
    return this.copy()
  }

  async simulateOutputRestored(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'outputUnavailable') {
      this.setSession('streaming')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
    }
    return this.copy()
  }

  async simulatePairingConsumed(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'consumed', expiresInSeconds: null }
      this.setSession('ready')
    }
    return this.copy()
  }

  async simulateMetricsBurst(): Promise<DesktopSnapshot> {
    const metrics = this.snapshotState.metrics
    metrics.authenticatedPackets = addToCounter(metrics.authenticatedPackets, 48)
    metrics.duplicatePackets = addToCounter(metrics.duplicatePackets, 2)
    metrics.rejectedPackets = addToCounter(metrics.rejectedPackets, 1)
    metrics.latePackets = addToCounter(metrics.latePackets, 5)
    metrics.reorderedPackets = addToCounter(metrics.reorderedPackets, 3)
    metrics.jitterResyncDroppedFrames = addToCounter(metrics.jitterResyncDroppedFrames, 2)
    metrics.plcFrames = addToCounter(metrics.plcFrames, 6)
    metrics.decodeErrors = addToCounter(metrics.decodeErrors, 1)
    metrics.pcmUnderflows = addToCounter(metrics.pcmUnderflows, 1)
    metrics.pcmOverflows = addToCounter(metrics.pcmOverflows, 1)
    metrics.pcmRefillLimitHits = addToCounter(metrics.pcmRefillLimitHits, 1)
    metrics.pcmDiscontinuities = addToCounter(metrics.pcmDiscontinuities, 1)

    const nextQueued = metrics.queuedPcmBlocks + 2
    metrics.queuedPcmHighWaterBlocks = Math.max(metrics.queuedPcmHighWaterBlocks, nextQueued)
    metrics.queuedPcmBlocks = Math.min(metrics.queuedPcmHighWaterBlocks, nextQueued)

    const nextJitterFrames = metrics.jitterFrames + 2
    metrics.jitterHighWaterFrames = Math.max(metrics.jitterHighWaterFrames, nextJitterFrames)
    metrics.jitterFrames = nextJitterFrames
    metrics.targetSmoothedJitterMs += 6
    metrics.targetFrames = Math.min(metrics.targetFrames + 1, metrics.targetMaxFrames)
    metrics.targetRaisedSteps += 1

    metrics.currentJitterMs += 14
    metrics.playoutDelayMs += 4

    return this.copy()
  }

  simulateCommandRejection(): Promise<DesktopSnapshot> {
    return Promise.reject(new Error('preview command rejection'))
  }

  async resetPreview(): Promise<DesktopSnapshot> {
    this.snapshotState = structuredClone(INITIAL_PREVIEW_SNAPSHOT)
    this.pairingExpiresAtMillis = null
    return this.copy()
  }

  private setSession(session: SessionState) {
    this.snapshotState.session = session
    this.snapshotState.output.streamActive = session === 'streaming'
  }

  private copy(): DesktopSnapshot {
    this.expireInvitationWhenDue(Date.now())
    return structuredClone(this.snapshotState)
  }

  private expireInvitationWhenDue(nowMillis: number) {
    if (this.snapshotState.pairing.state !== 'active' || this.pairingExpiresAtMillis === null) {
      return
    }

    const remainingMillis = this.pairingExpiresAtMillis - nowMillis
    if (remainingMillis <= 0) {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.setSession('idle')
      return
    }

    this.snapshotState.pairing = {
      state: 'active',
      expiresInSeconds: Math.ceil(remainingMillis / 1_000),
    }
  }
}

const PAIRING_INVITATION_LIFETIME_SECONDS = 300
const PAIRING_INVITATION_LIFETIME_MILLIS = PAIRING_INVITATION_LIFETIME_SECONDS * 1_000

export const desktopBridge: DesktopBridge = new PreviewDesktopBridge()

function addToCounter(counter: MetricCounter, delta: number): MetricCounter {
  if (!/^\d+$/.test(counter)) return counter
  return (BigInt(counter) + BigInt(delta)).toString()
}
