import type { DesktopSnapshot, OutputDevice } from './types'

export interface DesktopBridge {
  snapshot(): Promise<DesktopSnapshot>
  startStream(): Promise<DesktopSnapshot>
  stopStream(): Promise<DesktopSnapshot>
  selectOutput(deviceId: string): Promise<DesktopSnapshot>
  createPairing(): Promise<DesktopSnapshot>
  expirePairing(): Promise<DesktopSnapshot>
}

const previewOutput: OutputDevice[] = [
  { id: 'default', label: 'System default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
  { id: 'usb', label: 'USB audio output', sampleRateHz: 48_000, channels: 2, isDefault: false },
]

class PreviewDesktopBridge implements DesktopBridge {
  private pairingExpiresAtMillis: number | null = null

  private snapshotState: DesktopSnapshot = {
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
