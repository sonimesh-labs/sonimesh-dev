import type { DesktopSnapshot, MetricCounter, OutputDevice } from './types'

export interface DesktopBridge {
  snapshot(): Promise<DesktopSnapshot>
  startStream(): Promise<DesktopSnapshot>
  stopStream(): Promise<DesktopSnapshot>
  selectOutput(deviceId: string): Promise<DesktopSnapshot>
  createPairing(): Promise<DesktopSnapshot>
  expirePairing(): Promise<DesktopSnapshot>
  revokePairing(): Promise<DesktopSnapshot>
  /** Preview-only: simulates losing the control connection from a trusted session. */
  simulateConnectionLost(): Promise<DesktopSnapshot>
  /** Preview-only: simulates a reconnected control connection returning to ready. */
  simulateReconnected(): Promise<DesktopSnapshot>
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

export class PreviewDesktopBridge implements DesktopBridge {
  private pairingExpiresAtMillis: number | null = null

  private snapshotState: DesktopSnapshot = structuredClone(INITIAL_PREVIEW_SNAPSHOT)

  async snapshot(): Promise<DesktopSnapshot> {
    return this.copy()
  }

  async startStream(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'ready' && this.snapshotState.pairing.state === 'consumed') {
      this.snapshotState.session = 'streaming'
    }
    return this.copy()
  }

  async stopStream(): Promise<DesktopSnapshot> {
    if (['streaming', 'reconnecting', 'outputUnavailable'].includes(this.snapshotState.session)) {
      this.snapshotState.session = 'ready'
    }
    return this.copy()
  }

  async selectOutput(deviceId: string): Promise<DesktopSnapshot> {
    if (this.snapshotState.outputDevices.some((device) => device.id === deviceId)) {
      this.snapshotState.selectedOutputId = deviceId
    }
    return this.copy()
  }

  async createPairing(): Promise<DesktopSnapshot> {
    if (['inactive', 'expired', 'revoked'].includes(this.snapshotState.pairing.state)) {
      this.pairingExpiresAtMillis = Date.now() + PAIRING_INVITATION_LIFETIME_MILLIS
      this.snapshotState.pairing = { state: 'active', expiresInSeconds: PAIRING_INVITATION_LIFETIME_SECONDS }
      this.snapshotState.session = 'pairing'
    }
    return this.copy()
  }

  async expirePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.snapshotState.session = 'idle'
    }
    return this.copy()
  }

  async revokePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'consumed') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'revoked', expiresInSeconds: null }
      this.snapshotState.session = 'idle'
    }
    return this.copy()
  }

  async simulateConnectionLost(): Promise<DesktopSnapshot> {
    if (
      ['ready', 'streaming'].includes(this.snapshotState.session) &&
      this.snapshotState.pairing.state === 'consumed'
    ) {
      this.snapshotState.session = 'reconnecting'
    }
    return this.copy()
  }

  async simulateReconnected(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'reconnecting') {
      this.snapshotState.session = 'ready'
    }
    return this.copy()
  }

  async simulateOutputLost(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'streaming') {
      this.snapshotState.session = 'outputUnavailable'
    }
    return this.copy()
  }

  async simulateOutputRestored(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'outputUnavailable') {
      this.snapshotState.session = 'streaming'
    }
    return this.copy()
  }

  async simulatePairingConsumed(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.snapshotState.pairing = { state: 'consumed', expiresInSeconds: null }
      this.snapshotState.session = 'ready'
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
    metrics.pcmRefillLimitHits = addToCounter(metrics.pcmRefillLimitHits, 1)
    metrics.pcmDiscontinuities = addToCounter(metrics.pcmDiscontinuities, 1)

    const nextQueued = metrics.queuedPcmBlocks + 2
    metrics.queuedPcmHighWaterBlocks = Math.max(metrics.queuedPcmHighWaterBlocks, nextQueued)
    metrics.queuedPcmBlocks = Math.min(metrics.queuedPcmHighWaterBlocks, nextQueued)
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
      this.snapshotState.session = 'idle'
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
