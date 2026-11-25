import type { DesktopSnapshot, JitterMode } from './types'

export interface DesktopBridge {
  snapshot(): Promise<DesktopSnapshot>
  startStream(): Promise<DesktopSnapshot>
  stopStream(): Promise<DesktopSnapshot>
  selectOutput(deviceId: string | null): Promise<DesktopSnapshot>
  /** Re-enumerates desktop output devices and returns the updated snapshot. */
  refreshOutputDevices(): Promise<DesktopSnapshot>
  /** Plays a short test tone through the currently selected desktop output. */
  testOutput(): Promise<void>
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
