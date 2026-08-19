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

/** Structured protocol error code from the receiver's control plane. */
export type ControlErrorCode =
  | 'invalidRequest'
  | 'retryLater'
  | 'unsupportedCapability'
  | 'incompatibleVersion'
  | 'streamNotFound'

/** One receiver-generated control-plane reply mirroring the Rust ControlReply contract. */
export interface ControlReply {
  kind: 'streamStatus' | 'error'
  /** Echoes the answered command's request identifier. */
  requestId: number
  /** Bounded human-readable detail from the receiver. */
  detail: string
  /** Observed lifecycle state for a stream-status reply. */
  state?: SessionState
  /** Structured protocol error code for an error reply. */
  code?: ControlErrorCode
  /** Whether the sender may retry the same command unchanged. */
  retryable?: boolean
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
  /** Target-gated decode ticks in the active scheduler epoch. */
  targetGatedTicks: MetricCounter
  /** Below-target plays forced after the gate deadline in the active epoch. */
  targetForcedPlayouts: MetricCounter
  streamResets: MetricCounter
  nominalInputSampleRateHz: number
  nominalOutputSampleRateHz: number
  driftAdjustmentPpm: number
  driftHardResyncs: MetricCounter
  currentJitterMs: number
  playoutDelayMs: number
}

/** One issued diagnostic-export receipt mirroring the Rust ExportReceipt (ADR 0011). */
export interface DiagnosticExportReceipt {
  /** Monotonic export sequence starting at zero; encoded as a string so it never loses u64 precision. */
  sequence: string
  /** Serialized bundle byte size; encoded as a string so it never loses precision. */
  bytes: string
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
  /** Bounded milliseconds remaining before the control session expires while reconnecting; null otherwise. */
  reconnectRemainingMs: number | null
  metrics: StreamMetrics
  /** Bounded, most-recent-first receiver-generated control-plane replies. */
  replies: ControlReply[]
  /** Bounded, most-recent-first issued diagnostic-export receipts (preview-owned). */
  diagnosticExports: DiagnosticExportReceipt[]
  previewAdapter: boolean
}