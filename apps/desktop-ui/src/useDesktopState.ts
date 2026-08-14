import { computed, onMounted, ref } from 'vue'

import { desktopBridge } from './bridge'
import type { DesktopSnapshot } from './types'

export function useDesktopState() {
  const snapshot = ref<DesktopSnapshot | null>(null)
  const busy = ref(false)
  const error = ref<string | null>(null)

  const isStreaming = computed(() => snapshot.value?.session === 'streaming')

  async function run(command: () => Promise<DesktopSnapshot>) {
    if (busy.value) return

    busy.value = true
    error.value = null
    try {
      snapshot.value = await command()
    } catch {
      error.value = 'The desktop control service did not accept this command.'
    } finally {
      busy.value = false
    }
  }

  const refresh = () => run(() => desktopBridge.snapshot())
  const start = () => run(() => desktopBridge.startStream())
  const stop = () => run(() => desktopBridge.stopStream())
  const selectOutput = (id: string) => run(() => desktopBridge.selectOutput(id))
  const setGain = (gain: number) => run(() => desktopBridge.setGain(gain))
  const setMuted = (muted: boolean) => run(() => desktopBridge.setMuted(muted))
  const exportDiagnostics = () => run(() => desktopBridge.exportDiagnostics())
  const createPairing = () => run(() => desktopBridge.createPairing())
  const expirePairing = () => run(() => desktopBridge.expirePairing())
  const revokePairing = () => run(() => desktopBridge.revokePairing())
  const simulateConnectionLost = () => run(() => desktopBridge.simulateConnectionLost())
  const simulateReconnected = () => run(() => desktopBridge.simulateReconnected())
  const simulateSessionExpired = () => run(() => desktopBridge.simulateSessionExpired())
  const simulateOutputLost = () => run(() => desktopBridge.simulateOutputLost())
  const simulateOutputRestored = () => run(() => desktopBridge.simulateOutputRestored())
  const simulatePairingConsumed = () => run(() => desktopBridge.simulatePairingConsumed())
  const simulateMetricsBurst = () => run(() => desktopBridge.simulateMetricsBurst())
  const simulateCommandRejection = () => run(() => desktopBridge.simulateCommandRejection())
  const resetPreview = () => run(() => desktopBridge.resetPreview())

  onMounted(refresh)

  return {
    snapshot,
    busy,
    error,
    isStreaming,
    refresh,
    start,
    stop,
    selectOutput,
    setGain,
    setMuted,
    exportDiagnostics,
    createPairing,
    expirePairing,
    revokePairing,
    simulateConnectionLost,
    simulateReconnected,
    simulateSessionExpired,
    simulateOutputLost,
    simulateOutputRestored,
    simulatePairingConsumed,
    simulateMetricsBurst,
    simulateCommandRejection,
    resetPreview,
  }
}
