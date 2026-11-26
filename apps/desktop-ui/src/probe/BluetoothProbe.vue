<script lang="ts">
let pendingLoopbackSwitch: Promise<void> | undefined
</script>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'

type LoopbackSnapshot = { running: boolean; endpointId?: string | null; endpointName: string; sampleRate: number; channels: number; samples: number; peak: number; runPeak: number; error: string | null }
type LoopbackEndpoint = { id: string; name: string; isDefault: boolean; sampleRate?: number | null; channels?: number | null }
type PlaybackCapability = 'ApplicationPcmSource' | 'SystemRouted' | 'Unsupported'

const names = ref<string[]>([])
const candidateIds = ref<string[]>([])
const playbackCapability = ref<PlaybackCapability | null>(null)
const LOOPBACK_NO_SAMPLES_DELAY_MS = 5000
const generation = ref(0)
const selected = ref('')
const BLUETOOTH_SOURCE_KEY = 'sonimesh.bluetoothProbe.sourceId'
const COMBINED_MONITOR_KEY = 'sonimesh.bluetoothProbe.combinedMonitorOwned'
const BLUETOOTH_RECOVERY_KEY = 'sonimesh.bluetoothProbe.recoveryPending'
const loaded = ref(false)
const discovering = ref(false)
const busy = ref(false)
const phase = ref<'idle' | 'waiting' | 'open'>('idle')
const error = ref<string | null>(null)
const statusError = ref<string | null>(null)
const suggestPhoneInitiated = ref(false)
const message = ref<string | null>(null)
const loopback = ref<LoopbackSnapshot | null>(null)
const loopbackEndpoints = ref<LoopbackEndpoint[]>([])
const LOOPBACK_ENDPOINT_KEY = 'sonimesh.bluetoothProbe.loopbackEndpoint'
const FOLLOW_DEFAULT_KEY = 'sonimesh.bluetoothProbe.followDefault'

function readFollowDefault() {
  try { return localStorage.getItem(FOLLOW_DEFAULT_KEY) === 'true' } catch { return false }
}

const followDefault = ref(readFollowDefault())
let failedDefaultId: string | undefined

function changeFollowDefault() {
  try { localStorage.setItem(FOLLOW_DEFAULT_KEY, String(followDefault.value)) } catch { /* Optional preference. */ }
  failedDefaultId = undefined
  void refreshLoopbackEndpoints()
}

function readLoopbackEndpointId() {
  try {
    return localStorage.getItem(LOOPBACK_ENDPOINT_KEY) ?? ''
  } catch {
    return ''
  }
}

function rememberLoopbackEndpoint(event: Event) {
  loopbackSelectionEdited = true
  try {
    localStorage.setItem(LOOPBACK_ENDPOINT_KEY, (event.target as HTMLSelectElement).value)
  } catch {
    // Endpoint preference is optional; the current selection still works.
  }
  loopbackEndpointMessage.value = null
}

const loopbackEndpointId = ref(readLoopbackEndpointId())
const loopbackBusy = ref(false)
const loopbackError = ref<string | null>(null)
const loopbackStatusError = ref<string | null>(null)
const loopbackEndpointMessage = ref<string | null>(null)
const loopbackToneBusy = ref(false)
const loopbackToneMessage = ref<string | null>(null)
const observationMessage = ref<string | null>(null)
const loopbackNoSamplesLong = ref(false)
const combinedMonitorOwned = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined
let loopbackTimer: ReturnType<typeof setTimeout> | undefined
let loopbackStartedAt: number | undefined
let disposed = false
let statusRequest = 0
let loopbackRequest = 0
let refreshOnFocus = false
let focusRefreshPending = false
let recoverBluetoothWait = false
let loopbackSelectionEdited = false
let loopbackSwitch: Promise<void> | undefined

function sourceName() {
  return names.value[Number(selected.value)] ?? 'selected source'
}

function sourceOptionLabel(index: number, name: string) {
  const matchingIndexes = names.value.flatMap((candidate, candidateIndex) => candidate === name ? [candidateIndex] : [])
  return matchingIndexes.length > 1 ? `${name} (${matchingIndexes.indexOf(index) + 1})` : name
}

function readBluetoothSourceId() {
  try {
    return sessionStorage.getItem(BLUETOOTH_SOURCE_KEY) ?? undefined
  } catch {
    return undefined
  }
}

function rememberBluetoothSource() {
  try {
    const id = candidateIds.value[Number(selected.value)]
    if (id) sessionStorage.setItem(BLUETOOTH_SOURCE_KEY, id)
    else sessionStorage.removeItem(BLUETOOTH_SOURCE_KEY)
  } catch {
    // Session preference is optional; the current selection still works.
  }
}

function rememberBluetoothRecovery(shouldRecover: boolean) {
  recoverBluetoothWait = shouldRecover
  try {
    if (shouldRecover) sessionStorage.setItem(BLUETOOTH_RECOVERY_KEY, 'true')
    else sessionStorage.removeItem(BLUETOOTH_RECOVERY_KEY)
  } catch {
    // Recovery preference is optional; the current panel still tracks it.
  }
}

function readBluetoothRecovery() {
  try {
    return sessionStorage.getItem(BLUETOOTH_RECOVERY_KEY) === 'true'
  } catch {
    return false
  }
}

function rememberCombinedMonitorOwnership(owned: boolean) {
  combinedMonitorOwned.value = owned
  try {
    if (owned) sessionStorage.setItem(COMBINED_MONITOR_KEY, 'true')
    else sessionStorage.removeItem(COMBINED_MONITOR_KEY)
  } catch {
    // Session ownership is optional; the current panel still tracks it.
  }
}

function readCombinedMonitorOwnership() {
  try {
    return sessionStorage.getItem(COMBINED_MONITOR_KEY) === 'true'
  } catch {
    return false
  }
}

function playbackCapabilityDescription(capability: PlaybackCapability) {
  switch (capability) {
    case 'ApplicationPcmSource':
      return 'SoniMesh can access PCM from the source.'
    case 'SystemRouted':
      return 'Windows routes playback through the system; application PCM is unavailable.'
    case 'Unsupported':
      return 'No supported Bluetooth playback path is available on this platform.'
  }
}

function emptyPlaybackSourcesDescription(capability: PlaybackCapability | null) {
  if (capability === 'Unsupported') {
    return 'Bluetooth playback is unavailable on this platform.'
  }
  return 'No playback sources are currently listed. Check pairing and refresh; an empty list does not establish adapter support.'
}

function loopbackEndpointLabel(endpoint: LoopbackEndpoint) {
  const matchingIds = loopbackEndpoints.value.filter(candidate => candidate.name === endpoint.name).map(candidate => candidate.id).sort()
  const name = matchingIds.length > 1 ? `${endpoint.name} (${matchingIds.indexOf(endpoint.id) + 1})` : endpoint.name
  const details = [endpoint.isDefault ? 'default' : '', endpoint.sampleRate ? `${endpoint.sampleRate / 1000} kHz` : '', endpoint.channels ? `${endpoint.channels} ch` : '']
    .filter(Boolean)
  return details.length ? `${name} (${details.join(', ')})` : name
}

function selectAvailableLoopbackEndpoint(endpoints: LoopbackEndpoint[]) {
  const previousId = loopbackEndpointId.value
  const activeId = loopback.value?.running ? loopback.value.endpointId : null
  if (followDefault.value && !activeId) {
    const defaultEndpoint = endpoints.find(endpoint => endpoint.isDefault)
    loopbackEndpoints.value = endpoints
    loopbackEndpointId.value = defaultEndpoint?.id ?? ''
    loopbackEndpointMessage.value = defaultEndpoint ? null : 'Windows did not report a default output endpoint.'
    return
  }
  if (activeId) {
    loopbackEndpoints.value = endpoints
    if (!loopbackSelectionEdited || !endpoints.some(endpoint => endpoint.id === previousId)) {
      loopbackEndpointId.value = activeId
      loopbackSelectionEdited = false
    }
    loopbackEndpointMessage.value = endpoints.some(endpoint => endpoint.id === activeId)
      ? null : 'The monitored output endpoint is unavailable in the current device list.'
    return
  }
  const preferred = endpoints.find(endpoint => endpoint.id === readLoopbackEndpointId())
  if (preferred) {
    loopbackEndpoints.value = endpoints
    loopbackEndpointId.value = preferred.id
    loopbackEndpointMessage.value = null
    return
  }

  if (endpoints.some(endpoint => endpoint.id === previousId)) {
    loopbackEndpoints.value = endpoints
    loopbackEndpointMessage.value = readLoopbackEndpointId() && previousId !== readLoopbackEndpointId()
      ? `Saved output endpoint is unavailable. Using ${endpoints.find(endpoint => endpoint.id === previousId)?.name ?? 'the selected output'}.`
      : null
    return
  }

  const previousName = loopbackEndpoints.value.find(endpoint => endpoint.id === previousId)?.name ?? loopback.value?.endpointName
  const replacement = endpoints.find(endpoint => endpoint.isDefault) ?? endpoints[0]
  loopbackEndpoints.value = endpoints
  loopbackEndpointId.value = replacement?.id ?? ''
  if (!previousId) {
    loopbackEndpointMessage.value = null
  } else if (replacement) {
    loopbackEndpointMessage.value = `${previousName || 'Saved output endpoint'} is unavailable. Using ${replacement.name}.`
  } else {
    loopbackEndpointMessage.value = `${previousName || 'Saved output endpoint'} is unavailable, and no Windows output endpoints are available.`
  }
}

function updateLoopbackWaitState(snapshot: LoopbackSnapshot) {
  if (!snapshot.running || snapshot.samples > 0) {
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
    return
  }
  loopbackStartedAt ??= Date.now()
  loopbackNoSamplesLong.value = Date.now() - loopbackStartedAt >= LOOPBACK_NO_SAMPLES_DELAY_MS
}

async function copyBluetoothObservation() {
  const snapshot = loopback.value
  const lines = [
    'SoniMesh Bluetooth experiment',
    `Bluetooth connection: ${statusError.value ? `status unavailable (last reported: ${phase.value})` : phase.value}`,
    `Playback capability: ${playbackCapability.value ?? 'unknown'}`,
    `Windows output monitor: ${loopbackStatusError.value ? 'status unavailable' : snapshot ? (snapshot.running ? 'running' : 'stopped') : 'unknown'}`,
  ]
  if (snapshot && snapshot.sampleRate > 0) {
    lines.push(`Last reported format: ${snapshot.sampleRate} Hz, ${snapshot.channels} channels`)
    lines.push(`Last reported samples: ${snapshot.samples}`)
    lines.push(`Last reported recent peak: ${(snapshot.peak * 100).toFixed(1)}%`)
    lines.push(`Session peak: ${snapshot.runPeak == null ? 'unknown' : `${(snapshot.runPeak * 100).toFixed(1)}%`}`)
  }
  if (loopbackError.value || snapshot?.error) lines.push('Output monitor reported an error.')
  lines.push('PCM observations are from the Windows output mix and do not identify the Bluetooth source or prove audible playback.')
  try {
    if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(lines.join('\n'))
    observationMessage.value = 'Bluetooth observation copied.'
  } catch {
    observationMessage.value = 'Could not copy the observation. Use the visible status instead.'
  }
}

async function resumeBluetoothWait(request: number): Promise<'recovered' | 'failed' | false> {
  if (!recoverBluetoothWait || selected.value === '') return false
  busy.value = true
  try {
    await window.__TAURI__!.core.invoke('bluetooth_prepare', {
      generation: generation.value,
      index: Number(selected.value),
    })
    if (disposed || request !== statusRequest) return 'failed'
    phase.value = 'waiting'
    message.value = `Windows closed the playback connection. Waiting for ${sourceName()} to reconnect through Bluetooth.${combinedMonitorOwned.value ? ' Windows output monitoring remains active.' : ''}`
  } catch (cause) {
    if (disposed || request !== statusRequest) return 'failed'
    rememberBluetoothRecovery(false)
    phase.value = 'idle'
    error.value = `Windows closed the playback connection, but waiting could not restart: ${String(cause)}`
    await stopCombinedMonitor()
    return 'failed'
  } finally {
    if (!disposed && request === statusRequest) busy.value = false
  }
  return 'recovered'
}

async function refresh() {
  busy.value = true
  discovering.value = true
  error.value = null
  suggestPhoneInitiated.value = false
  const previousIndex = selected.value === '' ? -1 : Number(selected.value)
  const previousId = candidateIds.value[previousIndex] ?? readBluetoothSourceId()
  const previousName = names.value[previousIndex]
  const previousNameCount = previousName === undefined ? 0 : names.value.filter(name => name === previousName).length
  try {
    if (!window.__TAURI__) throw new Error('Desktop probe unavailable')
    const result = await window.__TAURI__.core.invoke<{ generation: number; names: string[]; ids?: string[]; capability: PlaybackCapability }>('bluetooth_candidates')
    names.value = result.names
    candidateIds.value = result.ids ?? []
    generation.value = result.generation
    playbackCapability.value = result.capability ?? 'Unsupported'
    const idMatches = previousId === undefined ? [] : candidateIds.value.flatMap((id, index) => id === previousId ? [index] : [])
    if (previousId !== undefined) {
      selected.value = idMatches.length === 1 ? String(idMatches[0]) : ''
    } else if (previousName === undefined) {
      selected.value = ''
    } else {
      const matches = result.names.flatMap((name, index) => name === previousName ? [index] : [])
      selected.value = previousNameCount === 1 && matches.length === 1 ? String(matches[0]) : ''
    }
    loaded.value = true
  } catch (cause) {
    error.value = String(cause)
  } finally {
    busy.value = false
    discovering.value = false
  }
}

async function openBluetoothSettings() {
  if (!window.__TAURI__) return
  error.value = null
  refreshOnFocus = true
  try {
    await window.__TAURI__.core.invoke('open_bluetooth_settings')
  } catch (cause) {
    refreshOnFocus = false
    error.value = String(cause)
  }
}

function refreshCandidatesOnFocus() {
  if (disposed || !refreshOnFocus) return
  refreshOnFocus = false
  focusRefreshPending = true
  refreshAfterFocusWhenIdle()
}

function refreshAfterFocusWhenIdle() {
  if (disposed || !focusRefreshPending || busy.value || phase.value !== 'idle') return
  focusRefreshPending = false
  void refresh()
  if (!loopback.value?.running && !loopbackBusy.value) void refreshLoopbackEndpoints()
}

watch([busy, phase], refreshAfterFocusWhenIdle)

async function poll() {
  if (disposed || phase.value === 'idle') return
  const request = statusRequest
  let shouldContinue = true
  try {
    const state = await window.__TAURI__!.core.invoke<'idle' | 'waiting' | 'open'>('bluetooth_status')
    if (disposed || request !== statusRequest) return
    statusError.value = null
    if (state === 'idle') {
      shouldContinue = false
      const recovery = await resumeBluetoothWait(request)
      if (disposed || request !== statusRequest || recovery === 'failed') return
      if (recovery === 'recovered') {
        shouldContinue = true
      } else {
        rememberBluetoothRecovery(false)
        phase.value = 'idle'
        message.value = 'Windows closed the playback connection.'
        await stopCombinedMonitor()
        return
      }
    } else {
      if (state === 'open' && phase.value === 'waiting') message.value = `System playback connection opened for ${sourceName()}.`
      phase.value = state
    }
  } catch (cause) {
    if (!disposed && request === statusRequest) statusError.value = String(cause)
  }
  if (!disposed && request === statusRequest && shouldContinue) timer = setTimeout(poll, 1000)
}

async function pollLoopback() {
  if (disposed || !loopback.value?.running) return
  const request = loopbackRequest
  try {
    const snapshot = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_status')
    if (disposed || request !== loopbackRequest) return
    loopback.value = snapshot
    loopbackStatusError.value = null
    updateLoopbackWaitState(snapshot)
    if (snapshot.running && followDefault.value && !loopbackBusy.value && !loopbackToneBusy.value && !busy.value) {
      let endpoints: LoopbackEndpoint[]
      try {
        endpoints = await window.__TAURI__!.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? []
      } catch (cause) {
        if (!disposed && request === loopbackRequest && followDefault.value) {
          loopbackEndpointMessage.value = `Windows output devices could not be checked: ${String(cause)}. The current PCM check continues.`
        }
        if (!disposed && request === loopbackRequest && loopback.value?.running) loopbackTimer = setTimeout(pollLoopback, 1000)
        return
      }
      if (disposed || request !== loopbackRequest) return
      if (!followDefault.value || loopbackBusy.value || loopbackToneBusy.value || busy.value) {
        if (loopback.value?.running) loopbackTimer = setTimeout(pollLoopback, 1000)
        return
      }
      loopbackEndpoints.value = endpoints
      const target = endpoints.find(endpoint => endpoint.isDefault)
      if (!target) {
        failedDefaultId = undefined
        loopbackEndpointMessage.value = 'Windows did not report a default output endpoint; the current PCM check continues.'
      } else {
        loopbackEndpointMessage.value = null
        if (failedDefaultId && failedDefaultId !== target.id) failedDefaultId = undefined
        if (target.id !== snapshot.endpointId && target.id !== failedDefaultId) {
          loopbackEndpointId.value = target.id
          loopbackSelectionEdited = true
          failedDefaultId = target.id
          await switchLoopbackEndpoint()
          if (!disposed && loopback.value?.endpointId === target.id) failedDefaultId = undefined
          return
        }
      }
    }
  } catch (cause) {
    if (!disposed && request === loopbackRequest) loopbackStatusError.value = String(cause)
    if (!disposed && request === loopbackRequest && loopback.value) {
      loopback.value = { ...loopback.value, error: String(cause) }
    }
  }
  if (!disposed && request === loopbackRequest && loopback.value?.running) loopbackTimer = setTimeout(pollLoopback, 1000)
}

async function refreshLoopbackEndpoints() {
  if (!window.__TAURI__) return
  failedDefaultId = undefined
  loopbackBusy.value = true
  loopbackError.value = null
  try {
    selectAvailableLoopbackEndpoint(await window.__TAURI__.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? [])
  } catch (cause) {
    loopbackError.value = String(cause)
  } finally {
    loopbackBusy.value = false
  }
}

async function recoverLoopback() {
  if (!window.__TAURI__) return
  const activeEndpointId = loopback.value?.running ? loopback.value.endpointId : null
  loopbackRequest++
  clearTimeout(loopbackTimer)
  loopbackBusy.value = true
  loopbackError.value = null
  loopbackStatusError.value = null
  try {
    loopback.value = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_stop')
    loopbackStatusError.value = null
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
    const endpoints = await window.__TAURI__.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? []
    selectAvailableLoopbackEndpoint(endpoints)
    if (activeEndpointId && endpoints.some(endpoint => endpoint.id === activeEndpointId)) {
      loopbackEndpointId.value = activeEndpointId
      loopbackEndpointMessage.value = null
    }
    if (!loopbackEndpointId.value) throw new Error('No Windows output endpoints are available.')
    const result = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: loopbackEndpointId.value })
    if (!result.running) throw new Error('Windows output monitoring did not restart.')
    loopback.value = result
    loopbackStartedAt = Date.now()
    loopbackNoSamplesLong.value = false
    void pollLoopback()
  } catch (cause) {
    loopbackError.value = String(cause)
    if (loopback.value?.running) void pollLoopback()
  } finally {
    loopbackBusy.value = false
  }
}

async function toggleLoopback() {
  if (!window.__TAURI__ || (!loopback.value?.running && !followDefault.value && !loopbackEndpointId.value)) return
  loopbackBusy.value = true
  loopbackError.value = null
  if (loopback.value?.running) {
    loopbackRequest++
    clearTimeout(loopbackTimer)
  }
  try {
    if (!loopback.value?.running && followDefault.value) {
      selectAvailableLoopbackEndpoint(await window.__TAURI__.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? [])
      if (disposed) return
      if (!loopbackEndpointId.value) throw new Error('Windows did not report a default output endpoint.')
    }
    const result = loopback.value?.running
      ? await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_stop')
      : await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: loopbackEndpointId.value })
    loopback.value = result
    if (result.running) {
      loopbackStartedAt = Date.now()
      loopbackNoSamplesLong.value = false
      void pollLoopback()
    } else {
      loopbackRequest++
      clearTimeout(loopbackTimer)
      loopbackStartedAt = undefined
      loopbackNoSamplesLong.value = false
      rememberCombinedMonitorOwnership(false)
    }
  } catch (cause) {
    loopbackError.value = String(cause)
    if (loopback.value?.running) void pollLoopback()
  } finally {
    loopbackBusy.value = false
  }
}

async function playLoopbackTestTone() {
  const endpointId = loopback.value?.endpointId || loopbackEndpointId.value
  if (!window.__TAURI__ || !loopback.value?.running || !endpointId) return
  loopbackToneBusy.value = true
  loopbackToneMessage.value = null
  loopbackError.value = null
  try {
    await window.__TAURI__.core.invoke('loopback_test_tone', { endpointId })
    loopbackToneMessage.value = 'Test tone finished.'
    loopbackRequest++
    clearTimeout(loopbackTimer)
    void pollLoopback()
  } catch (cause) {
    loopbackError.value = String(cause)
  } finally {
    loopbackToneBusy.value = false
  }
}

async function stopCombinedMonitor() {
  if (loopbackSwitch) await loopbackSwitch
  if (!combinedMonitorOwned.value || !window.__TAURI__) return
  loopbackRequest++
  clearTimeout(loopbackTimer)
  loopbackBusy.value = true
  try {
    loopback.value = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_stop')
    loopbackStartedAt = undefined
    loopbackNoSamplesLong.value = false
    rememberCombinedMonitorOwnership(false)
  } catch (cause) {
    loopbackError.value = String(cause)
    if (loopback.value?.running) void pollLoopback()
  } finally {
    loopbackBusy.value = false
  }
}

async function switchLoopbackEndpoint() {
  if (!window.__TAURI__ || !loopback.value?.running || !loopbackEndpointId.value || loopbackBusy.value) return
  const endpointId = loopbackEndpointId.value
  const previousEndpointId = loopback.value.endpointId
  loopbackRequest++
  clearTimeout(loopbackTimer)
  loopbackBusy.value = true
  loopbackError.value = null
  loopbackStatusError.value = null
  loopbackToneMessage.value = null
  loopbackSwitch = (async () => {
    let stopped = false
    try {
      loopback.value = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_stop')
      if (loopback.value.running) throw new Error('Windows output monitoring did not stop.')
      stopped = true
      loopbackStartedAt = undefined
      loopbackNoSamplesLong.value = false
      const result = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId })
      if (!result.running) throw new Error('Windows output monitoring did not start.')
      loopback.value = result
      loopbackSelectionEdited = false
      loopbackStartedAt = Date.now()
      void pollLoopback()
    } catch (cause) {
      let detail = String(cause)
      if (stopped && previousEndpointId) {
        try {
          const restored = await window.__TAURI__!.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: previousEndpointId })
          if (!restored.running) throw new Error('Windows output monitoring did not restart.')
          loopback.value = restored
          loopbackStartedAt = Date.now()
          loopbackNoSamplesLong.value = false
          detail += ` Previous output monitoring resumed on ${restored.endpointName}.`
        } catch (restoreCause) {
          detail += ` Previous output monitoring could not be restored: ${String(restoreCause)}`
        }
      }
      loopbackError.value = detail
      if (loopback.value?.running) void pollLoopback()
      else rememberCombinedMonitorOwnership(false)
    }
  })()
  pendingLoopbackSwitch = loopbackSwitch
  try {
    await loopbackSwitch
  } finally {
    if (pendingLoopbackSwitch === loopbackSwitch) pendingLoopbackSwitch = undefined
    loopbackSwitch = undefined
    loopbackBusy.value = false
  }
}

async function toggle(mode: 'connect' | 'wait' = 'connect', monitorOutput = false, fallbackToWait = false) {
  if (!window.__TAURI__) return
  busy.value = true
  if (monitorOutput) loopbackBusy.value = true
  error.value = null
  suggestPhoneInitiated.value = false
  message.value = null
  statusRequest++
  clearTimeout(timer)
  let loopbackStartedHere = false
  let bluetoothAttempted = false
  let directError: unknown
  try {
    if (phase.value !== 'idle') {
      rememberBluetoothRecovery(false)
      await window.__TAURI__.core.invoke('bluetooth_disconnect')
      phase.value = 'idle'
      clearTimeout(timer)
      await stopCombinedMonitor()
      message.value = 'Disconnected.'
    } else {
      if (monitorOutput && !loopback.value?.running) {
        loopbackError.value = null
        try {
          if (followDefault.value) {
            selectAvailableLoopbackEndpoint(await window.__TAURI__.core.invoke<LoopbackEndpoint[]>('loopback_endpoints') ?? [])
            if (disposed) return
            if (!loopbackEndpointId.value) throw new Error('Windows did not report a default output endpoint.')
          }
          loopback.value = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_start', { endpointId: loopbackEndpointId.value })
          if (!loopback.value.running) throw new Error('Windows output monitoring did not start.')
          loopbackStartedHere = true
          rememberCombinedMonitorOwnership(true)
          loopbackStartedAt = Date.now()
          loopbackNoSamplesLong.value = false
        } catch (cause) {
          loopbackError.value = String(cause)
          throw cause
        }
      }
      bluetoothAttempted = true
      let connectionMode = mode
      if (fallbackToWait && mode === 'connect') {
        try {
          await window.__TAURI__.core.invoke('bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
        } catch (cause) {
          directError = cause
          await window.__TAURI__.core.invoke('bluetooth_prepare', { generation: generation.value, index: Number(selected.value) })
          connectionMode = 'wait'
        }
      } else {
        await window.__TAURI__.core.invoke(mode === 'wait' ? 'bluetooth_prepare' : 'bluetooth_connect', { generation: generation.value, index: Number(selected.value) })
      }
      phase.value = connectionMode === 'wait' ? 'waiting' : 'open'
      rememberBluetoothRecovery(true)
      message.value = connectionMode === 'wait'
        ? `${directError ? `Direct connection failed (${String(directError)}). ` : ''}Waiting for the phone to connect to ${sourceName()} through Bluetooth.${monitorOutput ? ' Windows output monitoring is active.' : ''}`
        : `System playback connection opened for ${sourceName()}.`
      if (loopbackStartedHere) void pollLoopback()
      void poll()
    }
  } catch (cause) {
    rememberBluetoothRecovery(false)
    if (loopbackStartedHere) {
      await stopCombinedMonitor()
    }
    if (fallbackToWait && directError) {
      error.value = `Direct connection failed: ${String(directError)} Phone-initiated wait also failed: ${String(cause)}`
    } else if (monitorOutput && !bluetoothAttempted) {
      loopbackError.value ||= String(cause)
    } else {
      error.value = String(cause)
    }
    suggestPhoneInitiated.value = !fallbackToWait && mode === 'connect' && phase.value === 'idle'
    if (phase.value !== 'idle') void poll()
  } finally {
    busy.value = false
    if (monitorOutput) loopbackBusy.value = false
  }
}

onMounted(async () => {
  if (!window.__TAURI__) return
  window.addEventListener('focus', refreshCandidatesOnFocus)
  busy.value = true
  const request = statusRequest
  try {
    if (pendingLoopbackSwitch) {
      loopbackBusy.value = true
      try {
        await pendingLoopbackSwitch
      } finally {
        loopbackBusy.value = false
      }
      if (disposed || request !== statusRequest) return
    }
    recoverBluetoothWait = readBluetoothRecovery()
    const hadCombinedMonitorOwnership = readCombinedMonitorOwnership()
    rememberCombinedMonitorOwnership(hadCombinedMonitorOwnership)
    try {
      const state = await window.__TAURI__.core.invoke<'idle' | 'waiting' | 'open'>('bluetooth_status')
      if (disposed || request !== statusRequest) return
      statusError.value = null
      if (state !== 'idle') {
        phase.value = state
        rememberBluetoothRecovery(true)
        message.value = state === 'waiting'
          ? 'Waiting for the phone to connect through Bluetooth.'
          : 'System playback connection is open.'
      } else if (recoverBluetoothWait) {
        phase.value = 'waiting'
        message.value = 'Restoring phone-initiated Bluetooth wait.'
      }
    } catch (cause) {
      if (!disposed && request === statusRequest) {
        statusError.value = String(cause)
        if (recoverBluetoothWait) {
          phase.value = 'waiting'
          message.value = 'Restoring phone-initiated Bluetooth wait.'
        }
      }
    }
    try {
      const loopbackState = await window.__TAURI__.core.invoke<LoopbackSnapshot>('loopback_status')
      if (disposed || request !== statusRequest) return
      loopback.value = loopbackState
      loopbackStatusError.value = null
      rememberCombinedMonitorOwnership(loopbackState.running && hadCombinedMonitorOwnership)
      if (loopbackState.running) {
        loopbackStartedAt = Date.now()
        loopbackNoSamplesLong.value = false
        void pollLoopback()
      }
    } catch (cause) {
      if (!disposed && request === statusRequest) loopbackStatusError.value = String(cause)
    }
    await refresh()
    await refreshLoopbackEndpoints()
    if (!disposed && request === statusRequest && phase.value !== 'idle') void poll()
  } catch (cause) {
    if (!disposed && request === statusRequest) error.value = String(cause)
  } finally {
    if (!disposed && request === statusRequest) busy.value = false
  }
})

onBeforeUnmount(() => {
  disposed = true
  statusRequest++
  loopbackRequest++
  refreshOnFocus = false
  focusRefreshPending = false
  window.removeEventListener('focus', refreshCandidatesOnFocus)
  clearTimeout(timer)
  clearTimeout(loopbackTimer)
})
</script>

<template>
  <section class="panel utility-panel" aria-labelledby="bluetooth-title">
    <p class="eyebrow">Windows experiment</p>
    <h2 id="bluetooth-title">Bluetooth system playback</h2>
    <p class="preview-note">Connect a paired source through Windows. Audio is played by the system and does not enter the SoniMesh mixer.</p>
    <p v-if="playbackCapability" role="status">Capability: {{ playbackCapability }}. {{ playbackCapabilityDescription(playbackCapability) }}</p>
    <button type="button" class="preview-action" :disabled="busy" @click="openBluetoothSettings">Open Bluetooth settings</button>
    <button type="button" class="preview-action" :disabled="busy || phase !== 'idle'" @click="refresh">Refresh Bluetooth sources</button>
    <p v-if="discovering" role="status">{{ loaded ? 'Refreshing Bluetooth sources...' : 'Searching for Bluetooth sources...' }}</p>
    <p v-else-if="loaded && names.length === 0" role="status">{{ emptyPlaybackSourcesDescription(playbackCapability) }}</p>
    <template v-if="names.length">
      <label for="bluetooth-source">Playback source</label>
      <select id="bluetooth-source" v-model="selected" :disabled="busy || phase !== 'idle'" @change="rememberBluetoothSource">
        <option value="" disabled>Select a source</option>
        <option v-for="(name, index) in names" :key="index" :value="String(index)">{{ sourceOptionLabel(index, name) }}</option>
      </select>
    </template>
    <button v-if="names.length || phase !== 'idle'" type="button" class="preview-action" :disabled="busy || loopbackBusy || (phase === 'idle' && selected === '')" @click="toggle('connect')">{{ phase === 'idle' ? 'Connect Bluetooth' : 'Disconnect Bluetooth' }}</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || selected === ''" @click="toggle('connect', false, true)">Connect or wait for phone</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || selected === ''" @click="toggle('wait')">Wait for phone to connect</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || loopbackBusy || selected === '' || (!followDefault && !loopbackEndpointId)" @click="toggle('wait', true)">Wait + monitor system output</button>
    <button v-if="names.length && phase === 'idle'" type="button" class="preview-action" :disabled="busy || loopbackBusy || selected === '' || (!followDefault && !loopbackEndpointId)" @click="toggle('connect', true, true)">Connect + monitor (auto wait fallback)</button>
    <p v-if="message" role="status">{{ message }}</p>
    <p v-if="error" class="command-error" role="alert">{{ error }}</p>
    <p v-if="statusError" class="command-error" role="alert">Bluetooth status unavailable: {{ statusError }}</p>
    <p v-if="suggestPhoneInitiated" role="status">Try waiting for the phone to connect, then start playback from the phone.</p>
    <h3>System output PCM check</h3>
    <p class="preview-note">Measures the selected Windows playback mix; other apps' audio may also appear here.</p>
    <button type="button" class="preview-action" :disabled="loopbackBusy || loopbackToneBusy" @click="refreshLoopbackEndpoints">Refresh output devices</button>
    <label><input id="loopback-follow-default" v-model="followDefault" type="checkbox" :disabled="loopbackBusy || loopbackToneBusy" @change="changeFollowDefault"> Follow Windows default output</label>
    <label for="loopback-endpoint">Output endpoint</label>
    <select id="loopback-endpoint" v-model="loopbackEndpointId" :disabled="followDefault || loopbackBusy || loopbackToneBusy" @change="rememberLoopbackEndpoint">
      <option value="" disabled>Select an output endpoint</option>
      <option v-for="endpoint in loopbackEndpoints" :key="endpoint.id" :value="endpoint.id">{{ loopbackEndpointLabel(endpoint) }}</option>
    </select>
    <p v-if="loopbackEndpointMessage" role="status">{{ loopbackEndpointMessage }}</p>
    <button type="button" class="preview-action" :disabled="loopbackBusy || loopbackToneBusy || (!loopback?.running && !followDefault && !loopbackEndpointId)" @click="toggleLoopback">{{ loopbackBusy ? 'Working...' : loopback?.running ? 'Stop PCM check' : 'Start PCM check' }}</button>
    <button v-if="loopback?.running && !followDefault" type="button" class="preview-action" :disabled="loopbackBusy || loopbackToneBusy || !loopbackEndpointId || loopbackEndpointId === loopback.endpointId" @click="switchLoopbackEndpoint()">Switch PCM check to selected output</button>
    <button v-if="loopback?.error || loopbackError || loopbackStatusError" type="button" class="preview-action" :disabled="loopbackBusy || loopbackToneBusy" @click="recoverLoopback">Recover PCM check</button>
    <button type="button" class="preview-action" :disabled="loopbackBusy || loopbackToneBusy || !loopback?.running" @click="playLoopbackTestTone">{{ loopbackToneBusy ? 'Playing...' : 'Play test tone' }}</button>
    <p v-if="loopbackToneMessage" role="status">{{ loopbackToneMessage }}</p>
    <p v-if="loopback?.running && loopback.samples === 0" role="status">{{ loopbackNoSamplesLong ? 'No Windows audio samples have arrived yet. Start playback and check the selected output endpoint.' : 'Waiting for Windows audio samples.' }}</p>
    <div v-if="loopback && (loopback.running || loopback.sampleRate > 0)" class="pcm-level" role="meter" aria-label="Recent system output peak" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="(loopback.peak * 100).toFixed(1)" :aria-valuetext="`${(loopback.peak * 100).toFixed(1)} percent`">
      <span :style="{ width: `${Math.min(100, loopback.peak * 100)}%` }"></span>
    </div>
    <p v-if="loopback && (loopback.running || loopback.sampleRate > 0)" role="status">{{ loopback.endpointName }} · {{ loopback.sampleRate }} Hz, {{ loopback.channels }} channels; {{ loopback.samples }} samples; recent peak {{ (loopback.peak * 100).toFixed(1) }}%; session peak {{ ((loopback.runPeak ?? loopback.peak) * 100).toFixed(1) }}%<template v-if="loopback.samples > 0">; {{ loopback.peak > 0.001 ? 'PCM activity detected recently' : 'No recent PCM activity' }}</template>.</p>
    <p v-if="loopbackStatusError || loopbackError || loopback?.error" class="command-error" role="alert">{{ loopbackStatusError || loopbackError || loopback?.error }}</p>
    <button type="button" class="preview-action" :disabled="busy || loopbackBusy" @click="copyBluetoothObservation">Copy Bluetooth observation</button>
    <p v-if="observationMessage" role="status">{{ observationMessage }}</p>
  </section>
</template>
