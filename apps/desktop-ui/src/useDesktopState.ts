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
  const refreshingDevices = ref(false)
  let snapshotPollTimer: ReturnType<typeof setInterval> | undefined
  let snapshotRevision = 0

  async function run(
    command: () => Promise<DesktopSnapshot | void>,
    failureMessage = 'The desktop control service did not accept this command.',
  ) {
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
      error.value = failureMessage
    } finally {
      busy.value = false
    }
  }

  async function refresh() {
    if (busy.value || refreshing.value) return

    refreshing.value = true
    const requestRevision = snapshotRevision
    try {
      const nextSnapshot = await desktopBridge.snapshot()
      if (nextSnapshot !== undefined && requestRevision === snapshotRevision) {
        snapshot.value = nextSnapshot
        error.value = null
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
  const selectOutput = (id: string | null) => {
    outputTestStatus.value = null
    return run(
      () => desktopBridge.selectOutput(id),
      'Could not select this output device. Refresh the device list and try again.',
    )
  }
  async function refreshOutputDevices() {
    if (busy.value) return

    const previousSelectedOutputId = snapshot.value?.selectedOutputId ?? null
    refreshingDevices.value = true
    try {
      await run(async () => {
        const nextSnapshot = await desktopBridge.refreshOutputDevices()
        if (nextSnapshot.selectedOutputId !== previousSelectedOutputId) {
          outputTestStatus.value = null
        }
        return nextSnapshot
      }, 'Could not refresh output devices. Check the audio service and try again.')
    } finally {
      refreshingDevices.value = false
    }
  }
  const testOutput = () => {
    outputTestStatus.value = null
    return run(
      async () => {
        await desktopBridge.testOutput()
        outputTestStatus.value = 'Test tone completed on the selected output.'
      },
      'Could not play a test tone on the selected output. Check the device and try again.',
    )
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
    refreshOutputDevices,
    refreshingDevices,
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
