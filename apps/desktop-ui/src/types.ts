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

export interface StreamMetrics {
  authenticatedPackets: number
  rejectedPackets: number
  plcFrames: number
  pcmUnderflows: number
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
