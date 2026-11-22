import type {
  DesktopBridge,
  DesktopBridgeInvoke,
} from './bridge-contract'
import type { DesktopSnapshot, JitterMode } from './types'

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
