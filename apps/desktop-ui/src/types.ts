export type SessionState = 'idle' | 'pairing' | 'ready' | 'streaming' | 'reconnecting' | 'outputUnavailable'

export interface OutputDevice {
  id: string
  label: string
  sampleRateHz: number
  channels: number
  isDefault: boolean
}

export interface PairingStatus {
  state: 'inactive' | 'active' | 'consumed' | 'expired'
  expiresInSeconds: number | null
}

/** Unsigned cumulative Rust counter encoded without JavaScript number loss. */
export type MetricCounter = string

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
  pcmOverflows: MetricCounter
  pcmDiscontinuities: MetricCounter
  queuedPcmBlocks: number
  queuedPcmHighWaterBlocks: number
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
  pairing: PairingStatus
  metrics: StreamMetrics
  previewAdapter: boolean
}
