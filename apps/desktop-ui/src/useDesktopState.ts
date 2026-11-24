import { onBeforeUnmount, onMounted, ref } from 'vue'

import { desktopBridge } from './bridge'
import type { DesktopSnapshot } from './types'
import type { JitterMode } from './types'

const SNAPSHOT_POLL_INTERVAL_MS = 1_000

export function useDesktopState() {
  const snapshot = ref<DesktopSnapshot | null>(null)
  const busy = ref(false)
  const refreshing = ref(false)
  const error = ref<string | null>(null)
  const outputTestStatus = ref<string | null>(null)
  let snapshotPollTimer: ReturnType<typeof setInterval> | undefined
  let snapshotRevision = 0

  async function run(command: () => Promise<DesktopSnapshot | void>) {
    if (busy.value) return

    snapshotRevision += 1
    busy.value = true
    error.value = null
    try {
      const nextSnapshot = await command()
      if (nextSnapshot !== undefined) {
        snapshot.value = nextSnapshot
      }
    } catch {
      error.value = 'The desktop control service did not accept this command.'
    } finally {
      busy.value = false
    }
  }

  async function refresh() {
    if (busy.value || refreshing.value) return

    refreshing.value = true
    error.value = null
    const requestRevision = snapshotRevision
    try {
      const nextSnapshot = await desktopBridge.snapshot()
      if (nextSnapshot !== undefined && requestRevision === snapshotRevision) {
        snapshot.value = nextSnapshot
      }
    } catch {
      if (requestRevision === snapshotRevision) {
        error.value = 'The desktop control service did not accept this command.'
      }
    } finally {
      refreshing.value = false
    }
  }
  const start = () => run(() => desktopBridge.startStream())
  const stop = () => run(() => desktopBridge.stopStream())
  const selectOutput = (id: string) => {
    outputTestStatus.value = null
    return run(() => desktopBridge.selectOutput(id))
  }
  const testOutput = () => {
    outputTestStatus.value = null
    return run(async () => {
      await desktopBridge.testOutput()
      outputTestStatus.value = 'Test tone completed on the selected output.'
    })
  }
  const setGain = (gain: number) => run(() => desktopBridge.setGain(gain))
  const setMuted = (muted: boolean) => run(() => desktopBridge.setMuted(muted))
  const setJitterMode = (mode: JitterMode) => run(() => desktopBridge.setJitterMode(mode))
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

  onMounted(() => {
    void refresh()
    snapshotPollTimer = setInterval(() => {
      void refresh()
    }, SNAPSHOT_POLL_INTERVAL_MS)
  })

  onBeforeUnmount(() => {
    if (snapshotPollTimer !== undefined) {
      clearInterval(snapshotPollTimer)
      snapshotPollTimer = undefined
    }
  })

  return {
    snapshot,
    busy,
    refreshing,
    error,
    outputTestStatus,
    refresh,
    start,
    stop,
    selectOutput,
    testOutput,
    setGain,
    setMuted,
    setJitterMode,
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
