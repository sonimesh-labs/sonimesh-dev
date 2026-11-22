import type {
  ControlErrorCode,
  ControlReply,
  DesktopSnapshot,
  DiagnosticExportReceipt,
  MetricCounter,
  JitterMode,
  OutputDevice,
  SessionState,
  StreamMetrics,
} from './types'

export interface DesktopBridge {
  snapshot(): Promise<DesktopSnapshot>
  startStream(): Promise<DesktopSnapshot>
  stopStream(): Promise<DesktopSnapshot>
  selectOutput(deviceId: string): Promise<DesktopSnapshot>
  /** Publishes a bounded linear gain target; out-of-range or non-finite values are ignored. */
  setGain(gain: number): Promise<DesktopSnapshot>
  /** Publishes the per-stream mute flag. */
  setMuted(muted: boolean): Promise<DesktopSnapshot>
  /** Selects the bounded latency/jitter target preset. */
  setJitterMode(mode: JitterMode): Promise<DesktopSnapshot>
  /** Issues one bounded diagnostic export and returns the updated snapshot. */
  exportDiagnostics(): Promise<DesktopSnapshot>
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

/** Minimal Tauri v2 IPC surface kept injectable for browser and unit tests. */
export type DesktopBridgeInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

/**
 * Low-rate production adapter for the future Tauri shell.
 *
 * The adapter owns no state and never carries media frames. The shell owns the
 * receiver session and returns the same redacted snapshot shape used by the
 * preview bridge.
 */
export class TauriDesktopBridge implements DesktopBridge {
  constructor(private readonly invoke: DesktopBridgeInvoke) {}

  snapshot(): Promise<DesktopSnapshot> {
    return this.call('desktop_snapshot')
  }

  startStream(): Promise<DesktopSnapshot> {
    return this.call('desktop_start_stream')
  }

  stopStream(): Promise<DesktopSnapshot> {
    return this.call('desktop_stop_stream')
  }

  selectOutput(deviceId: string): Promise<DesktopSnapshot> {
    return this.call('desktop_select_output', { deviceId })
  }

  setGain(gain: number): Promise<DesktopSnapshot> {
    return this.call('desktop_set_gain', { gain })
  }

  setMuted(muted: boolean): Promise<DesktopSnapshot> {
    return this.call('desktop_set_muted', { muted })
  }

  setJitterMode(mode: JitterMode): Promise<DesktopSnapshot> {
    return this.call('desktop_set_jitter_mode', { mode })
  }

  exportDiagnostics(): Promise<DesktopSnapshot> {
    return this.call('desktop_export_diagnostics')
  }

  createPairing(): Promise<DesktopSnapshot> {
    return this.call('desktop_create_pairing')
  }

  expirePairing(): Promise<DesktopSnapshot> {
    return this.call('desktop_expire_pairing')
  }

  revokePairing(): Promise<DesktopSnapshot> {
    return this.call('desktop_revoke_pairing')
  }

  simulateConnectionLost(): Promise<DesktopSnapshot> {
    return this.unsupported('connection-loss simulation')
  }

  simulateReconnected(): Promise<DesktopSnapshot> {
    return this.unsupported('reconnect simulation')
  }

  simulateSessionExpired(): Promise<DesktopSnapshot> {
    return this.unsupported('session-expiry simulation')
  }

  simulateOutputLost(): Promise<DesktopSnapshot> {
    return this.unsupported('output-loss simulation')
  }

  simulateOutputRestored(): Promise<DesktopSnapshot> {
    return this.unsupported('output-restore simulation')
  }

  simulatePairingConsumed(): Promise<DesktopSnapshot> {
    return this.unsupported('pairing-consumption simulation')
  }

  simulateMetricsBurst(): Promise<DesktopSnapshot> {
    return this.unsupported('metrics simulation')
  }

  simulateCommandRejection(): Promise<DesktopSnapshot> {
    return this.unsupported('command-rejection simulation')
  }

  resetPreview(): Promise<DesktopSnapshot> {
    return this.unsupported('preview reset')
  }

  private async call(command: string, args?: Record<string, unknown>): Promise<DesktopSnapshot> {
    const snapshot = await this.invoke<DesktopSnapshot>(command, args)
    return { ...snapshot, previewAdapter: false }
  }

  private unsupported(action: string): Promise<DesktopSnapshot> {
    return Promise.reject(new Error(`${action} is only available in the preview adapter`))
  }
}

interface TauriGlobalRuntime {
  core: {
    invoke: DesktopBridgeInvoke
  }
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobalRuntime
  }
}

const previewOutput: OutputDevice[] = [
  { id: 'default', label: 'System default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
  { id: 'usb', label: 'USB audio output', sampleRateHz: 48_000, channels: 2, isDefault: false },
]

/** Hard bound on the retained receiver-generated reply log. */
const MAX_PREVIEW_REPLIES = 6

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
  reconnectRemainingMs: null,
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
    targetGatedTicks: '12',
    targetForcedPlayouts: '1',
    streamResets: '3',
    nominalInputSampleRateHz: 48_000,
    nominalOutputSampleRateHz: 48_000,
    driftAdjustmentPpm: 110,
    driftHardResyncs: '1',
    currentJitterMs: 38,
    playoutDelayMs: 64,
  },
  replies: [],
  diagnosticExports: [],
  previewAdapter: true,
}

export class PreviewDesktopBridge implements DesktopBridge {
  private pairingExpiresAtMillis: number | null = null

  /** Last observed wall-clock instant while the preview invitation was active. */
  private pairingLastObservedAtMillis: number | null = null

  /** Preview clock instant when the bounded reconnect window was armed. */
  private reconnectArmedAtMillis: number | null = null

  /** Last observed wall-clock instant while the reconnect window was active. */
  private reconnectLastObservedAtMillis: number | null = null

  private nextRequestId = 1

  /** Monotonic preview export sequence, mirroring the Rust journal's u64 receipt. */
  private nextExportSequence = 0

  private snapshotState: DesktopSnapshot = structuredClone(INITIAL_PREVIEW_SNAPSHOT)

  async snapshot(): Promise<DesktopSnapshot> {
    return this.copy()
  }

  async startStream(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'ready' && this.snapshotState.pairing.state === 'consumed') {
      this.setSession('streaming')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
      this.recordStatus('streaming', 'stream started')
    } else {
      this.recordError('start requires a trusted ready session')
    }
    return this.copy()
  }

  async stopStream(): Promise<DesktopSnapshot> {
    if (['streaming', 'reconnecting', 'outputUnavailable'].includes(this.snapshotState.session)) {
      this.setSession('ready')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
      this.recordStatus('ready', 'stream stopped')
    } else {
      this.recordError('no active stream to stop')
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

  async setJitterMode(mode: JitterMode): Promise<DesktopSnapshot> {
    const preset = JITTER_MODE_PRESETS[mode]
    if (preset === undefined) return this.copy()
    Object.assign(this.snapshotState.metrics, preset)
    return this.copy()
  }

  async exportDiagnostics(): Promise<DesktopSnapshot> {
    const receipt: DiagnosticExportReceipt = {
      sequence: String(this.nextExportSequence),
      bytes: String(PREVIEW_EXPORT_BYTES),
    }
    this.nextExportSequence += 1
    // The ADR 0011 journal keeps the newest receipts under a fixed retention
    // bound while the sequence itself keeps increasing monotonically.
    this.snapshotState.diagnosticExports = [receipt, ...this.snapshotState.diagnosticExports].slice(
      0,
      MAX_PREVIEW_DIAGNOSTIC_EXPORTS,
    )
    return this.copy()
  }

  async createPairing(): Promise<DesktopSnapshot> {
    if (['inactive', 'expired', 'revoked'].includes(this.snapshotState.pairing.state)) {
      const nowMillis = Date.now()
      this.pairingExpiresAtMillis = nowMillis + PAIRING_INVITATION_LIFETIME_MILLIS
      this.pairingLastObservedAtMillis = nowMillis
      this.snapshotState.pairing = { state: 'active', expiresInSeconds: PAIRING_INVITATION_LIFETIME_SECONDS }
      this.setSession('pairing')
      this.recordStatus('pairing', 'pairing invitation opened')
    } else {
      this.recordError('pairing already in progress')
    }
    return this.copy()
  }

  async expirePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.pairingLastObservedAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.setSession('idle')
      this.recordStatus('idle', 'pairing invitation expired')
    } else {
      this.recordError('no active invitation to expire')
    }
    return this.copy()
  }

  async revokePairing(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'consumed') {
      this.pairingExpiresAtMillis = null
      this.pairingLastObservedAtMillis = null
      this.snapshotState.pairing = { state: 'revoked', expiresInSeconds: null }
      this.setSession('idle')
      this.recordStatus('idle', 'trusted session revoked')
    } else {
      this.recordError('no trusted session to revoke')
    }
    return this.copy()
  }

  async simulateConnectionLost(): Promise<DesktopSnapshot> {
    if (
      ['ready', 'streaming'].includes(this.snapshotState.session) &&
      this.snapshotState.pairing.state === 'consumed'
    ) {
      this.setSession('reconnecting')
      const nowMillis = Date.now()
      this.reconnectArmedAtMillis = nowMillis
      this.reconnectLastObservedAtMillis = nowMillis
      this.recordStatus('reconnecting', 'control connection lost')
    } else {
      this.recordError('connection loss requires a trusted session')
    }
    return this.copy()
  }

  async simulateReconnected(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'reconnecting') {
      this.setSession('ready')
      this.recordStatus('ready', 'control connection restored')
    } else {
      this.recordError('no reconnecting session to restore')
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
      this.pairingLastObservedAtMillis = null
      this.setSession('idle')
      this.recordStatus('idle', 'trusted session expired')
    } else {
      this.recordError('session expiry requires a trusted session')
    }
    return this.copy()
  }

  async simulateOutputLost(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'streaming') {
      this.setSession('outputUnavailable')
      this.snapshotState.output.faultReported = true
      this.snapshotState.output.health = 'streamError'
      this.recordStatus('outputUnavailable', 'output device lost')
    } else {
      this.recordError('output loss requires a streaming session')
    }
    return this.copy()
  }

  async simulateOutputRestored(): Promise<DesktopSnapshot> {
    if (this.snapshotState.session === 'outputUnavailable') {
      this.setSession('streaming')
      this.snapshotState.output.faultReported = false
      this.snapshotState.output.health = 'healthy'
      this.recordStatus('streaming', 'output device restored')
    } else {
      this.recordError('output restore requires an unavailable output')
    }
    return this.copy()
  }

  async simulatePairingConsumed(): Promise<DesktopSnapshot> {
    if (this.snapshotState.pairing.state === 'active') {
      this.pairingExpiresAtMillis = null
      this.pairingLastObservedAtMillis = null
      this.snapshotState.pairing = { state: 'consumed', expiresInSeconds: null }
      this.setSession('ready')
      this.recordStatus('ready', 'sender consumed the pairing invitation')
    } else {
      this.recordError('no active invitation to consume')
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
    metrics.targetGatedTicks = addToCounter(metrics.targetGatedTicks, 2)
    metrics.targetForcedPlayouts = addToCounter(metrics.targetForcedPlayouts, 1)

    metrics.currentJitterMs += 14
    metrics.playoutDelayMs += 4

    this.recordStatus(this.snapshotState.session, 'metrics burst applied')
    return this.copy()
  }

  simulateCommandRejection(): Promise<DesktopSnapshot> {
    this.recordError('preview command rejection', 'retryLater', true)
    return Promise.reject(new Error('preview command rejection'))
  }

  async resetPreview(): Promise<DesktopSnapshot> {
    this.snapshotState = structuredClone(INITIAL_PREVIEW_SNAPSHOT)
    this.pairingExpiresAtMillis = null
    this.pairingLastObservedAtMillis = null
    this.reconnectArmedAtMillis = null
    this.reconnectLastObservedAtMillis = null
    this.nextRequestId = 1
    this.nextExportSequence = 0
    return this.copy()
  }

  private setSession(session: SessionState) {
    this.snapshotState.session = session
    this.snapshotState.output.streamActive = session === 'streaming'
    // The reconnect window is only meaningful while the session reconnects;
    // leaving that state clears the bounded remaining countdown and its clock.
    if (session !== 'reconnecting') {
      this.snapshotState.reconnectRemainingMs = null
      this.reconnectArmedAtMillis = null
      this.reconnectLastObservedAtMillis = null
    }
  }

  private recordReply(reply: Omit<ControlReply, 'requestId'>): void {
    this.snapshotState.replies = [
      { ...reply, requestId: this.nextRequestId },
      ...this.snapshotState.replies,
    ].slice(0, MAX_PREVIEW_REPLIES)
    this.nextRequestId += 1
  }

  private recordStatus(state: SessionState, detail: string): void {
    this.recordReply({ kind: 'streamStatus', state, detail })
  }

  private recordError(detail: string, code: ControlErrorCode = 'invalidRequest', retryable = false): void {
    this.recordReply({ kind: 'error', code, retryable, detail })
  }

  private copy(): DesktopSnapshot {
    const nowMillis = Date.now()
    this.expireInvitationWhenDue(nowMillis)
    this.refreshReconnectCountdown(nowMillis)
    return structuredClone(this.snapshotState)
  }

  private expireInvitationWhenDue(nowMillis: number) {
    if (this.snapshotState.pairing.state !== 'active' || this.pairingExpiresAtMillis === null) {
      return
    }

    if (
      this.pairingLastObservedAtMillis !== null &&
      nowMillis < this.pairingLastObservedAtMillis
    ) {
      this.pairingExpiresAtMillis = null
      this.pairingLastObservedAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.setSession('idle')
      return
    }
    this.pairingLastObservedAtMillis = nowMillis

    const remainingMillis = this.pairingExpiresAtMillis - nowMillis
    if (remainingMillis <= 0) {
      this.pairingExpiresAtMillis = null
      this.pairingLastObservedAtMillis = null
      this.snapshotState.pairing = { state: 'expired', expiresInSeconds: 0 }
      this.setSession('idle')
      return
    }

    this.snapshotState.pairing = {
      state: 'active',
      expiresInSeconds: Math.ceil(remainingMillis / 1_000),
    }
  }

  private refreshReconnectCountdown(nowMillis: number) {
    if (this.reconnectArmedAtMillis === null || this.snapshotState.session !== 'reconnecting') {
      return
    }

    if (
      this.reconnectLastObservedAtMillis !== null &&
      nowMillis < this.reconnectLastObservedAtMillis
    ) {
      // Match the Rust reconnect-grace owner: a clock regression expires the
      // session so a later forward jump cannot revive trusted state.
      this.reconnectArmedAtMillis = null
      this.reconnectLastObservedAtMillis = null
      this.setSession('idle')
      return
    }
    this.reconnectLastObservedAtMillis = nowMillis

    const remainingMillis = RECONNECT_GRACE_MILLIS - (nowMillis - this.reconnectArmedAtMillis)
    if (remainingMillis <= 0) {
      // The reconnect deadline passed without a reconnect; the control session
      // retires exactly as the Rust snapshot entry point projects. Expiry is a
      // local lifecycle event, so no receiver-generated reply is recorded.
      this.reconnectArmedAtMillis = null
      this.reconnectLastObservedAtMillis = null
      this.setSession('idle')
      return
    }

    this.snapshotState.reconnectRemainingMs = remainingMillis
  }
}

const PAIRING_INVITATION_LIFETIME_SECONDS = 300
const PAIRING_INVITATION_LIFETIME_MILLIS = PAIRING_INVITATION_LIFETIME_SECONDS * 1_000

/** Fixed preview reconnect window in milliseconds until a simulated reconnect. */
const RECONNECT_GRACE_MILLIS = 30_000

/** Fixed preview bundle size mirroring the Rust MAX_EXPORT_BYTES (16 KiB). */
const PREVIEW_EXPORT_BYTES = 16 * 1024

/** Fixed preview retention bound mirroring the Rust DEFAULT_MAX_RETAINED_EXPORTS. */
const MAX_PREVIEW_DIAGNOSTIC_EXPORTS = 8

const JITTER_MODE_PRESETS: Record<JitterMode, Pick<StreamMetrics, 'targetMinFrames' | 'targetMaxFrames' | 'targetFrames' | 'playoutDelayMs'>> = {
  lowLatency: { targetMinFrames: 1, targetMaxFrames: 2, targetFrames: 2, playoutDelayMs: 40 },
  balanced: { targetMinFrames: 2, targetMaxFrames: 4, targetFrames: 3, playoutDelayMs: 60 },
  stable: { targetMinFrames: 4, targetMaxFrames: 6, targetFrames: 5, playoutDelayMs: 100 },
}

/** Selects the live shell bridge when Tauri exposes its global API. */
export function createDesktopBridge(invoke?: DesktopBridgeInvoke): DesktopBridge {
  const runtimeInvoke = invoke ?? (typeof window === 'undefined' ? undefined : window.__TAURI__?.core?.invoke)
  return runtimeInvoke === undefined ? new PreviewDesktopBridge() : new TauriDesktopBridge(runtimeInvoke)
}

export const desktopBridge: DesktopBridge = createDesktopBridge()

function addToCounter(counter: MetricCounter, delta: number): MetricCounter {
  if (!/^\d+$/.test(counter)) return counter
  return (BigInt(counter) + BigInt(delta)).toString()
}
