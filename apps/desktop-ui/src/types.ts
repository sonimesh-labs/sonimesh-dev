export type SessionState = 'idle' | 'pairing' | 'ready' | 'streaming' | 'reconnecting' | 'outputUnavailable'

export interface OutputDevice {
  id: string
  label: string
  sampleRateHz: number
  channels: number
  isDefault: boolean
}

export type PairingState = 'inactive' | 'active' | 'consumed' | 'expired' | 'revoked'

export interface PairingStatus {
  state: PairingState
  expiresInSeconds: number | null
}

/** Unsigned cumulative Rust counter encoded without JavaScript number loss. */
export type MetricCounter = string

export type OutputHealth = 'healthy' | 'callbackCapacityExceeded' | 'streamError'

export interface OutputSnapshot {
  /** Whether a recovery output selection is retained by the controller. */
  selectionRetained: boolean
  /** Whether the controller currently owns an active output stream. */
  streamActive: boolean
  /** Whether a non-healthy output fault has already been reported. */
  faultReported: boolean
  /** Latest low-rate output health state. */
  health: OutputHealth
}

export interface StreamMetrics {
  authenticatedPackets: MetricCounter
  duplicatePackets: MetricCounter
  rejectedPackets: MetricCounter
  latePackets: MetricCounter
  reorderedPackets: MetricCounter
  jitterResyncDroppedFrames: MetricCounter
  plcFrames: MetricCounter
  decodeErrors: MetricCounter
  pcmUnderflows: MetricCounter
  pcmRefillLimitHits: MetricCounter
  pcmOverflows: MetricCounter
  pcmDiscontinuities: MetricCounter
  queuedPcmBlocks: number
  queuedPcmHighWaterBlocks: number
  /** Adaptive jitter target state published by the stream snapshot (ADR 0016). */
  jitterFrames: number
  jitterHighWaterFrames: number
  targetMinFrames: number
  targetMaxFrames: number
  targetFrames: number
  targetSmoothedJitterMs: number
  targetRaisedSteps: number
  targetLoweredSteps: number
  streamResets: MetricCounter
  nominalInputSampleRateHz: number
  nominalOutputSampleRateHz: number
  driftAdjustmentPpm: number
  driftHardResyncs: MetricCounter
  currentJitterMs: number
  playoutDelayMs: number
}

export interface DesktopSnapshot {
  session: SessionState
  outputDevices: OutputDevice[]
  selectedOutputId: string | null
  /** Linear per-stream gain target in 0.0..=1.0, attenuation only. */
  gain: number
  /** Mute flag published to the realtime render source. */
  muted: boolean
  /** Redacted output lifecycle mirrored from the Rust control snapshot. */
  output: OutputSnapshot
  pairing: PairingStatus
  metrics: StreamMetrics
  previewAdapter: boolean
}
