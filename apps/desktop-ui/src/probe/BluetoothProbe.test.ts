import { afterEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import BluetoothProbe from './BluetoothProbe.vue'

const originalClipboard = navigator.clipboard

afterEach(() => {
  delete window.__TAURI__
  vi.useRealTimers()
  sessionStorage.removeItem('sonimesh.bluetoothProbe.sourceId')
  sessionStorage.removeItem('sonimesh.bluetoothProbe.combinedMonitorOwned')
  sessionStorage.removeItem('sonimesh.bluetoothProbe.recoveryPending')
  localStorage.removeItem('sonimesh.bluetoothProbe.loopbackEndpoint')
  localStorage.removeItem('sonimesh.bluetoothProbe.followDefault')
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: originalClipboard })
})

it('follows default output only when enabled and remembers the preference', async () => {
  vi.useFakeTimers()
  const snapshot = (id: string, running = true) => ({ running, endpointId: id, endpointName: id, sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
  let active = 'old'
  let defaultId = 'old'
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(['old', 'new'].map(id => ({ id, name: id, isDefault: id === defaultId })))
    if (command === 'loopback_status') return Promise.resolve(snapshot(active))
    if (command === 'loopback_stop') return Promise.resolve(snapshot(active, false))
    if (command === 'loopback_start') { active = args!.endpointId!; return Promise.resolve(snapshot(active)) }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  defaultId = 'new'
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  await wrapper.get('#loopback-follow-default').setValue(true)
  await flushPromises()
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  expect(localStorage.getItem('sonimesh.bluetoothProbe.followDefault')).toBe('true')
  wrapper.unmount()
  const reopened = mount(BluetoothProbe)
  await flushPromises()
  expect((reopened.get('#loopback-follow-default').element as HTMLInputElement).checked).toBe(true)
  reopened.unmount()
})

it('resolves the current default when starting a PCM check', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  let defaultId = 'old'
  const stopped = { running: false, endpointId: null, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, runPeak: 0, error: null }
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(['old', 'new'].map(id => ({ id, name: id, isDefault: id === defaultId })))
    if (command === 'loopback_status') return Promise.resolve(stopped)
    if (command === 'loopback_start') return Promise.resolve({ ...stopped, running: true, endpointId: args?.endpointId })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  defaultId = 'new'
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  wrapper.unmount()
})

it('rechecks a missing default on start and allows retry when an output appears', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  let available = false
  const stopped = { running: false, endpointId: null, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, runPeak: 0, error: null }
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(available ? [{ id: 'new', name: 'New output', isDefault: true }] : [])
    if (command === 'loopback_status') return Promise.resolve(stopped)
    if (command === 'loopback_start') return Promise.resolve({ ...stopped, running: true, endpointId: args?.endpointId })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  const start = wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!
  expect(start.attributes('disabled')).toBeUndefined()
  await start.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Windows did not report a default output endpoint.')
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', expect.anything())
  available = true
  await start.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  wrapper.unmount()
})

it('rechecks a missing default when starting combined Bluetooth monitoring', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  let available = false
  const stopped = { running: false, endpointId: null, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, runPeak: 0, error: null }
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Phone'], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(available ? [{ id: 'new', name: 'New output', isDefault: true }] : [])
    if (command === 'loopback_status') return Promise.resolve(stopped)
    if (command === 'loopback_start') return Promise.resolve({ ...stopped, running: true, endpointId: args?.endpointId })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  const start = wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!
  expect(start.attributes('disabled')).toBeUndefined()
  await start.trigger('click')
  await flushPromises()
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_prepare', expect.anything())
  expect(wrapper.text()).toContain('Windows did not report a default output endpoint.')
  available = true
  await start.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 1, index: 0 })
  wrapper.unmount()
})

it('restores the old output after a failed default switch and retries on refresh', async () => {
  vi.useFakeTimers()
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  const snapshot = (id: string, running = true) => ({ running, endpointId: id, endpointName: id, sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
  let active = 'old'
  let defaultId = 'old'
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(['old', 'new'].map(id => ({ id, name: id, isDefault: id === defaultId })))
    if (command === 'loopback_status') return Promise.resolve(snapshot(active))
    if (command === 'loopback_stop') return Promise.resolve(snapshot(active, false))
    if (command === 'loopback_start') {
      if (args?.endpointId === 'new') return Promise.reject(new Error('unavailable'))
      active = 'old'; return Promise.resolve(snapshot(active))
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  defaultId = 'new'
  await vi.advanceTimersByTimeAsync(3000)
  expect(invoke.mock.calls.filter(([command, args]) => command === 'loopback_start' && args?.endpointId === 'new')).toHaveLength(1)
  expect(wrapper.text()).toContain('Previous output monitoring resumed on old')
  await wrapper.findAll('button').find(button => button.text() === 'Refresh output devices')!.trigger('click')
  await flushPromises()
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke.mock.calls.filter(([command, args]) => command === 'loopback_start' && args?.endpointId === 'new')).toHaveLength(2)
  wrapper.unmount()
})

it('keeps monitoring when the default is missing or output enumeration fails', async () => {
  vi.useFakeTimers()
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  const snapshot = { running: true, endpointId: 'old', endpointName: 'old', sampleRate: 48000, channels: 2, samples: 10, peak: 0, runPeak: 0, error: null }
  let state: 'normal' | 'missing' | 'error' = 'normal'
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') {
      if (state === 'error') return Promise.reject(new Error('enumeration failed'))
      return Promise.resolve([{ id: 'old', name: 'old', isDefault: state === 'normal' }])
    }
    if (command === 'loopback_status') return Promise.resolve(snapshot)
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  state = 'missing'
  await vi.advanceTimersByTimeAsync(1000)
  expect(wrapper.text()).toContain('Windows did not report a default output endpoint; the current PCM check continues.')
  state = 'error'
  await vi.advanceTimersByTimeAsync(1000)
  expect(wrapper.text()).toContain('Windows output devices could not be checked: Error: enumeration failed')
  expect(wrapper.text()).not.toContain('Bluetooth status unavailable')
  expect(wrapper.find('.command-error').exists()).toBe(false)
  state = 'normal'
  await vi.advanceTimersByTimeAsync(1000)
  expect(wrapper.text()).not.toContain('Windows output devices could not be checked')
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  wrapper.unmount()
})

it('does not switch after the monitor is stopped during default enumeration', async () => {
  vi.useFakeTimers()
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  const snapshot = { running: true, endpointId: 'old', endpointName: 'old', sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null }
  let defer = false
  let finish!: (value: object[]) => void
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return defer ? new Promise(resolve => { finish = resolve }) : Promise.resolve([{ id: 'old', name: 'old', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve(snapshot)
    if (command === 'loopback_stop') return Promise.resolve({ ...snapshot, running: false })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  defer = true
  await vi.advanceTimersByTimeAsync(1000)
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  finish([{ id: 'new', name: 'new', isDefault: true }])
  await flushPromises()
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  wrapper.unmount()
})

it('stops a combined monitor after Bluetooth recovery fails during an automatic switch', async () => {
  vi.useFakeTimers()
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  sessionStorage.setItem('sonimesh.bluetoothProbe.combinedMonitorOwned', 'true')
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'phone')
  const snapshot = (id: string, running = true) => ({ running, endpointId: id, endpointName: id, sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
  let phase = 'open'
  let active = 'old'
  let defaultId = 'old'
  let finishSwitch!: (value: object) => void
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Phone'], ids: ['phone'], capability: 'SystemRouted' })
    if (command === 'bluetooth_prepare') return Promise.reject(new Error('Phone unavailable'))
    if (command === 'loopback_endpoints') return Promise.resolve(['old', 'new'].map(id => ({ id, name: id, isDefault: id === defaultId })))
    if (command === 'loopback_status') return Promise.resolve(snapshot(active, !!active))
    if (command === 'loopback_stop') { const previous = active; active = ''; return Promise.resolve(snapshot(previous, false)) }
    if (command === 'loopback_start' && args?.endpointId === 'new') return new Promise(resolve => { finishSwitch = resolve })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  defaultId = 'new'
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  phase = 'idle'
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_stop')).toHaveLength(1)
  active = 'new'
  finishSwitch(snapshot('new'))
  await flushPromises()
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_stop')).toHaveLength(2)
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  expect(wrapper.text()).toContain('Phone unavailable')
  wrapper.unmount()
})

it.each(['stop', 'start'] as const)('restores combined monitoring after reopening during switch %s', async delayedStage => {
  vi.useFakeTimers()
  localStorage.setItem('sonimesh.bluetoothProbe.followDefault', 'true')
  sessionStorage.setItem('sonimesh.bluetoothProbe.combinedMonitorOwned', 'true')
  const snapshot = (id: string, running = true) => ({ running, endpointId: id, endpointName: id, sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
  let active = 'old'
  let defaultId = 'old'
  let release!: () => void
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve(['old', 'new'].map(id => ({ id, name: id, isDefault: id === defaultId })))
    if (command === 'loopback_status') return Promise.resolve(snapshot(active, !!active))
    if (command === 'loopback_stop') {
      const previous = active
      active = ''
      if (delayedStage === 'stop' && previous === 'old') return new Promise(resolve => { release = () => resolve(snapshot(previous, false)) })
      return Promise.resolve(snapshot(previous, false))
    }
    if (command === 'loopback_start') {
      if (delayedStage === 'start' && args?.endpointId === 'new') return new Promise(resolve => { release = () => { active = 'new'; resolve(snapshot(active)) } })
      active = args!.endpointId!
      return Promise.resolve(snapshot(active))
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const first = mount(BluetoothProbe)
  await flushPromises()
  defaultId = 'new'
  await vi.advanceTimersByTimeAsync(1000)
  expect(release).toBeTypeOf('function')
  first.unmount()
  const statusCalls = invoke.mock.calls.filter(([command]) => command === 'loopback_status').length
  const reopened = mount(BluetoothProbe)
  await flushPromises()
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_status')).toHaveLength(statusCalls)
  expect(reopened.findAll('button').find(button => button.text() === 'Working...')?.attributes('disabled')).toBeDefined()
  release()
  await flushPromises()
  expect(reopened.text()).toContain('new · 48000 Hz')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBe('true')
  await reopened.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(active).toBe('')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  reopened.unmount()
})

it('switches the monitored output while keeping the Bluetooth connection open', async () => {
  const old = { running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 100, peak: 0.2, runPeak: 0.2, error: null }
  const next = { ...old, endpointId: 'new', endpointName: 'Headphones', samples: 0, peak: 0, runPeak: 0 }
  let current = old
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }, { id: 'new', name: 'Headphones', isDefault: false }])
    if (command === 'loopback_status') return Promise.resolve(current)
    if (command === 'loopback_stop') return Promise.resolve({ ...old, running: false })
    if (command === 'loopback_start') { current = next; return Promise.resolve(next) }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('new')
  await wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_disconnect')
  expect(wrapper.text()).toContain('Headphones · 48000 Hz')
  expect(wrapper.text()).toContain('0 samples')
  wrapper.unmount()
})

it('shows a failed output switch and allows the selected endpoint to be retried', async () => {
  const old = { running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 100, peak: 0, runPeak: 0, error: null }
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }, { id: 'new', name: 'Headphones', isDefault: false }])
    if (command === 'loopback_status') return Promise.resolve(old)
    if (command === 'loopback_stop') return Promise.resolve({ ...old, running: false })
    if (command === 'loopback_start') return Promise.reject(new Error('Endpoint unavailable'))
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('new')
  await wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Endpoint unavailable')
  expect(wrapper.text()).toContain('Previous output monitoring could not be restored')
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'old' })
  expect(wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.attributes('disabled')).toBeUndefined()
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('new')
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_disconnect')
  wrapper.unmount()
})

it('restores the previous output when a switch fails and keeps the new selection for retry', async () => {
  const old = { running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 100, peak: 0.2, runPeak: 0.2, error: null }
  const restarted = { ...old, samples: 0, peak: 0, runPeak: 0 }
  sessionStorage.setItem('sonimesh.bluetoothProbe.combinedMonitorOwned', 'true')
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }, { id: 'new', name: 'Headphones', isDefault: false }])
    if (command === 'loopback_status') return Promise.resolve(old)
    if (command === 'loopback_stop') return Promise.resolve({ ...old, running: false })
    if (command === 'loopback_start') return args?.endpointId === 'new' ? Promise.reject(new Error('Endpoint unavailable')) : Promise.resolve(restarted)
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('new')
  await wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'old' })
  expect(wrapper.text()).toContain('Previous output monitoring resumed on Speakers')
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('new')
  expect(wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.attributes('disabled')).toBeUndefined()
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBe('true')
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_disconnect')
  wrapper.unmount()
})

it('keeps polling the old output when stopping it for a switch fails', async () => {
  const old = { running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 100, peak: 0, runPeak: 0, error: null }
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }, { id: 'new', name: 'Headphones', isDefault: false }])
    if (command === 'loopback_status') return Promise.resolve(old)
    if (command === 'loopback_stop') return Promise.reject(new Error('Stop failed'))
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('new')
  await wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Stop failed')
  expect(wrapper.text()).toContain('Speakers · 48000 Hz')
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', { endpointId: 'new' })
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_status').length).toBeGreaterThanOrEqual(3)
  wrapper.unmount()
})

it('stops the switched combined monitor if Bluetooth recovery fails during the switch', async () => {
  vi.useFakeTimers()
  let phase = 'idle'
  let prepareCalls = 0
  let active = ''
  let completeSwitch!: (snapshot: object) => void
  const snapshot = (id: string, running: boolean) => ({ running, endpointId: id, endpointName: id, sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
  const invoke = vi.fn((command: string, args?: { endpointId?: string }) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Phone'], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }, { id: 'new', name: 'Headphones', isDefault: false }])
    if (command === 'loopback_status') return Promise.resolve(snapshot(active, !!active))
    if (command === 'bluetooth_prepare') {
      if (++prepareCalls === 1) { phase = 'waiting'; return Promise.resolve() }
      return Promise.reject(new Error('Phone unavailable'))
    }
    if (command === 'loopback_stop') { const previous = active; active = ''; return Promise.resolve(snapshot(previous, false)) }
    if (command === 'loopback_start') {
      if (args?.endpointId === 'new') return new Promise(resolve => { completeSwitch = resolve })
      active = 'old'
      return Promise.resolve(snapshot(active, true))
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('new')
  await wrapper.findAll('button').find(button => button.text() === 'Switch PCM check to selected output')!.trigger('click')
  await flushPromises()
  phase = 'idle'
  await vi.advanceTimersByTimeAsync(1000)
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_stop')).toHaveLength(1)
  active = 'new'
  completeSwitch(snapshot('new', true))
  await flushPromises()
  expect(invoke.mock.calls.filter(([command]) => command === 'loopback_stop')).toHaveLength(2)
  expect(active).toBe('')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  expect(wrapper.text()).toContain('Phone unavailable')
  wrapper.unmount()
})

it('copies a source-neutral Bluetooth and system output observation', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Private phone'], ids: ['private-id'], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'private-endpoint', name: 'Private speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointId: 'private-endpoint', endpointName: 'Private speakers', sampleRate: 48000, channels: 2, samples: 4096, peak: 0.04, runPeak: 0.5, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Copy Bluetooth observation')!.trigger('click')
  const observation = writeText.mock.calls[0][0] as string
  expect(observation).toContain('Bluetooth connection: open')
  expect(observation).toContain('Playback capability: SystemRouted')
  expect(observation).toContain('Last reported samples: 4096')
  expect(observation).toContain('Last reported recent peak: 4.0%')
  expect(observation).toContain('Session peak: 50.0%')
  expect(observation).toContain('do not identify the Bluetooth source')
  expect(observation).not.toMatch(/Private|private-/)
  expect(wrapper.text()).toContain('Bluetooth observation copied.')
  wrapper.unmount()
})

it('reports unavailable status and clipboard failure when copying an observation', async () => {
  const writeText = vi.fn().mockRejectedValue(new Error('Clipboard unavailable'))
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status' || command === 'loopback_status') return Promise.reject(new Error('Unavailable'))
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_endpoints') return Promise.resolve([])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Copy Bluetooth observation')!.trigger('click')
  expect(writeText.mock.calls[0][0]).toContain('Bluetooth connection: status unavailable')
  expect(writeText.mock.calls[0][0]).toContain('Windows output monitor: status unavailable')
  expect(wrapper.text()).toContain('Could not copy the observation.')
  wrapper.unmount()
})

it('opens Windows Bluetooth settings from the probe', async () => {
  let endpointRefreshes = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Phone'] })
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve(endpointRefreshes === 1 ? [] : [{ id: 'bluetooth-output', name: 'Bluetooth output', isDefault: true }])
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_candidates')
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone')
  await wrapper.findAll('button').find(button => button.text() === 'Open Bluetooth settings')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('open_bluetooth_settings')
  expect(wrapper.find('[role="alert"]').exists()).toBe(false)
  window.dispatchEvent(new Event('focus'))
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_candidates')
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone')
  expect(endpointRefreshes).toBe(2)
  expect(wrapper.get('#loopback-endpoint').text()).toContain('Bluetooth output')
  wrapper.unmount()
})

it('defers the settings return refresh until an in-flight connection is disconnected', async () => {
  let finishConnect!: () => void
  let state = 'idle'
  let refreshes = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(state)
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve([])
    if (command === 'bluetooth_candidates') {
      refreshes++
      return Promise.resolve({ generation: refreshes, names: ['Phone'], ids: ['phone'], capability: 'SystemRouted' })
    }
    if (command === 'bluetooth_connect') return new Promise<void>(resolve => { finishConnect = () => { state = 'open'; resolve() } })
    if (command === 'bluetooth_disconnect') { state = 'idle'; return Promise.resolve() }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Open Bluetooth settings')!.trigger('click')
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Connect Bluetooth')!.trigger('click')
  window.dispatchEvent(new Event('focus'))
  await flushPromises()
  expect(refreshes).toBe(1)
  expect(wrapper.findAll('button').find(button => button.text() === 'Connect Bluetooth')!.attributes('disabled')).toBeDefined()
  finishConnect()
  await flushPromises()
  expect(refreshes).toBe(1)
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(refreshes).toBe(2)
  expect((wrapper.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('0')
  wrapper.unmount()
})

it('refreshes output choices after settings while Bluetooth and monitoring stay active', async () => {
  let endpointRefreshes = 0
  let candidateRefreshes = 0
  const snapshot = { running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 20, peak: 0.1, runPeak: 0.1, error: null }
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('open')
    if (command === 'loopback_status') return Promise.resolve(snapshot)
    if (command === 'bluetooth_candidates') {
      candidateRefreshes++
      return Promise.resolve({ generation: 1, names: ['Phone'], ids: ['phone'], capability: 'SystemRouted' })
    }
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve([
        { id: 'old', name: 'Speakers', isDefault: true },
        ...(endpointRefreshes > 1 ? [{ id: 'new', name: 'New headphones', isDefault: false }] : []),
      ])
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Open Bluetooth settings')!.trigger('click')
  window.dispatchEvent(new Event('focus'))
  await flushPromises()
  expect(endpointRefreshes).toBe(2)
  expect(candidateRefreshes).toBe(1)
  expect(wrapper.get('#loopback-endpoint').text()).toContain('New headphones')
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('old')
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_disconnect')
  wrapper.unmount()
})

it('defers the settings output refresh until a PCM check finishes starting', async () => {
  let finishStart!: () => void
  let endpointRefreshes = 0
  let started = false
  const stopped = { running: false, endpointId: null, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, runPeak: 0, error: null }
  const running = { ...stopped, running: true, endpointId: 'old', endpointName: 'Speakers', sampleRate: 48000, channels: 2 }
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability: 'SystemRouted' })
    if (command === 'loopback_status') return Promise.resolve(started ? running : stopped)
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve([{ id: 'old', name: 'Speakers', isDefault: true }])
    }
    if (command === 'loopback_start') return new Promise(resolve => { finishStart = () => { started = true; resolve(running) } })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await wrapper.findAll('button').find(button => button.text() === 'Open Bluetooth settings')!.trigger('click')
  window.dispatchEvent(new Event('focus'))
  await flushPromises()
  expect(endpointRefreshes).toBe(1)
  finishStart()
  await flushPromises()
  expect(endpointRefreshes).toBe(2)
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Stop PCM check')
  wrapper.unmount()
})

it('loads Bluetooth sources and output endpoints when Bluetooth status is unavailable', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.reject(new Error('Bluetooth status unavailable.'))
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 3, names: ['Phone'], ids: ['phone-id'], capability: 'SystemRouted' })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-output', name: 'Speakers', isDefault: true }])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone')
  expect(wrapper.get('#loopback-endpoint').text()).toContain('Speakers')
  expect(wrapper.text()).toContain('Bluetooth status unavailable.')
  expect(invoke).toHaveBeenCalledWith('bluetooth_candidates')
  expect(invoke).toHaveBeenCalledWith('loopback_endpoints')
  wrapper.unmount()
})

it('loads Bluetooth sources and output endpoints when loopback status is unavailable', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 4, names: ['Phone'], ids: ['phone-id'], capability: 'SystemRouted' })
    if (command === 'loopback_status') return Promise.reject(new Error('Output monitor status unavailable.'))
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-output', name: 'Speakers', isDefault: true }])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone')
  expect(wrapper.get('#loopback-endpoint').text()).toContain('Speakers')
  expect(wrapper.text()).toContain('Output monitor status unavailable.')
  expect(invoke).toHaveBeenCalledWith('bluetooth_candidates')
  expect(invoke).toHaveBeenCalledWith('loopback_endpoints')
  wrapper.unmount()
})

it.each([
  ['ApplicationPcmSource', 'SoniMesh can access PCM from the source.'],
  ['SystemRouted', 'Windows routes playback through the system; application PCM is unavailable.'],
  ['Unsupported', 'No supported Bluetooth playback path is available on this platform.'],
])('explains the %s Bluetooth playback capability', async (capability, description) => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [], capability })
    if (command === 'loopback_endpoints') return Promise.resolve([])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.text()).toContain(`Capability: ${capability}. ${description}`)
  if (capability === 'Unsupported') {
    expect(wrapper.text()).toContain('Bluetooth playback is unavailable on this platform.')
    expect(wrapper.text()).not.toContain('Pair a compatible device in Windows')
  }
  wrapper.unmount()
})

it('shows discovery progress until Bluetooth candidates return', async () => {
  let finishDiscovery!: (result: { generation: number; names: string[] }) => void
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return new Promise<{ generation: number; names: string[] }>(resolve => { finishDiscovery = resolve })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.text()).toContain('Searching for Bluetooth sources...')
  finishDiscovery({ generation: 1, names: ['Phone'] })
  await flushPromises()
  expect(wrapper.text()).not.toContain('Searching for Bluetooth sources...')
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone')
  wrapper.unmount()
})

it('restores the selected PCM endpoint when reopening the Bluetooth probe', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve([
      { id: 'default-output', name: 'Speakers', isDefault: true },
      { id: 'bluetooth-output', name: 'Bluetooth headphones', isDefault: false },
    ])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const first = mount(BluetoothProbe)
  await flushPromises()
  await first.get('#loopback-endpoint').setValue('bluetooth-output')
  expect(localStorage.getItem('sonimesh.bluetoothProbe.loopbackEndpoint')).toBe('bluetooth-output')
  first.unmount()

  const reopened = mount(BluetoothProbe)
  await flushPromises()
  expect((reopened.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('bluetooth-output')
  reopened.unmount()
})

it('uses the active monitored endpoint for a test tone after reopening', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.loopbackEndpoint', 'saved-output')
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointId: 'active-output', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve([
      { id: 'saved-output', name: 'Speakers', isDefault: true },
      { id: 'active-output', name: 'Speakers', isDefault: false },
    ])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('active-output')
  await wrapper.findAll('button').find(button => button.text() === 'Play test tone')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_test_tone', { endpointId: 'active-output' })
  expect(localStorage.getItem('sonimesh.bluetoothProbe.loopbackEndpoint')).toBe('saved-output')
  wrapper.unmount()
})

it('recovers the active monitored endpoint before the saved preference', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.loopbackEndpoint', 'saved-output')
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointId: 'active-output', endpointName: 'Active', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: 'Stream failed.' })
    if (command === 'loopback_endpoints') return Promise.resolve([
      { id: 'saved-output', name: 'Saved', isDefault: true },
      { id: 'active-output', name: 'Active', isDefault: false },
    ])
    if (command === 'loopback_stop') return Promise.resolve({ running: false, endpointId: 'active-output', endpointName: 'Active', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') return Promise.resolve({ running: true, endpointId: 'active-output', endpointName: 'Active', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Recover PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'active-output' })
  wrapper.unmount()
})

it('reports the fallback when the remembered PCM endpoint is unavailable', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.loopbackEndpoint', 'removed-output')
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-output', name: 'Speakers', isDefault: true }])
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('default-output')
  expect(wrapper.text()).toContain('Saved output endpoint is unavailable. Using Speakers.')
  expect(localStorage.getItem('sonimesh.bluetoothProbe.loopbackEndpoint')).toBe('removed-output')
  wrapper.unmount()
})

it('restores the remembered PCM endpoint when it reappears after refresh', async () => {
  localStorage.setItem('sonimesh.bluetoothProbe.loopbackEndpoint', 'bluetooth-output')
  let endpointRefreshes = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve(endpointRefreshes === 1
        ? [{ id: 'default-output', name: 'Speakers', isDefault: true }]
        : [
            { id: 'default-output', name: 'Speakers', isDefault: true },
            { id: 'bluetooth-output', name: 'Bluetooth headphones', isDefault: false },
          ])
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('default-output')
  expect(wrapper.text()).toContain('Saved output endpoint is unavailable. Using Speakers.')

  await wrapper.findAll('button').find(button => button.text() === 'Refresh output devices')!.trigger('click')
  await flushPromises()
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('bluetooth-output')
  expect(wrapper.text()).toContain('Bluetooth headphones')
  expect(wrapper.text()).not.toContain('Saved output endpoint is unavailable.')
  wrapper.unmount()
})

it('preserves the selected source by identity when discovery order changes', async () => {
  let generation = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') {
      generation++
      return Promise.resolve({
        generation,
        names: generation === 1 ? ['Phone', 'Headset'] : ['Headset', 'Phone'],
        ids: generation === 1 ? ['phone-id', 'headset-id'] : ['headset-id', 'phone-id'],
      })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  const refreshButton = wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!
  await refreshButton.trigger('click')
  await flushPromises()
  expect((wrapper.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('1')
  wrapper.unmount()
})

it('restores the selected Bluetooth source when reopening the panel', async () => {
  let generation = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') {
      generation++
      return Promise.resolve(generation === 1
        ? { generation, names: ['Phone', 'Headset'], ids: ['phone-id', 'headset-id'] }
        : { generation, names: ['Headset', 'Phone'], ids: ['headset-id', 'phone-id'] })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const first = mount(BluetoothProbe)
  await flushPromises()
  await first.get('#bluetooth-source').setValue('0')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.sourceId')).toBe('phone-id')
  expect(first.get('#bluetooth-source').text()).not.toContain('phone-id')
  first.unmount()

  const reopened = mount(BluetoothProbe)
  await flushPromises()
  expect((reopened.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('1')
  expect(reopened.get('#bluetooth-source').text()).toContain('Phone')
  reopened.unmount()
})

it('clears a stale source preference when the current bridge has no candidate IDs', async () => {
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'old-device-id')
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: ['Phone'] })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.sourceId')).toBeNull()
  wrapper.unmount()
})

it('labels duplicate source names and preserves the selected device across refresh', async () => {
  let generation = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') {
      generation++
      return Promise.resolve({ generation, names: ['Phone', 'Phone'], ids: ['phone-a', 'phone-b'] })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  const refreshButton = wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!
  await refreshButton.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('1')
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone (1)')
  expect(wrapper.get('#bluetooth-source').text()).toContain('Phone (2)')
  await refreshButton.trigger('click')
  await flushPromises()
  expect((wrapper.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('1')
  wrapper.unmount()
})

it('clears a selected device when it disappears even if another device has the same name', async () => {
  let generation = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, runPeak: 0, error: null })
    if (command === 'bluetooth_candidates') {
      generation++
      return Promise.resolve(generation === 1
        ? { generation, names: ['Phone', 'Phone'], ids: ['phone-a', 'phone-b'] }
        : { generation, names: ['Phone'], ids: ['phone-b'] })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  expect((wrapper.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('')
  wrapper.unmount()
})

it('refreshes candidates and connects and disconnects the selected source', async () => {
  let connected = false
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 3, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(connected ? 'open' : 'idle')
    if (command === 'bluetooth_connect') connected = true
    if (command === 'bluetooth_disconnect') connected = false
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('select').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Connect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_connect', { generation: 3, index: 0 })
  expect(wrapper.text()).toContain('System playback connection opened for Phone.')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_disconnect')
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('reports empty discovery and system closure', async () => {
  let empty = true
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: empty ? [] : ['Phone'], capability: 'SystemRouted' })
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('No playback sources are currently listed. Check pairing and refresh; an empty list does not establish adapter support.')
  empty = false
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('select').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Connect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Windows closed the playback connection.')
  wrapper.unmount()
})

it('suggests the phone-initiated path after a direct connection fails', async () => {
  let phase = 'idle'
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 2, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_connect') return Promise.reject(new Error('Windows could not open the connection.'))
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'bluetooth_disconnect') phase = 'idle'
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Connect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Try waiting for the phone to connect, then start playback from the phone.')
  await wrapper.findAll('button').find(button => button.text() === 'Wait for phone to connect')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 2, index: 0 })
  expect(wrapper.text()).toContain('Waiting for the phone to connect to Phone')
  expect(wrapper.text()).not.toContain('Try waiting for the phone to connect')
  wrapper.unmount()
})

it('restores an existing connection and allows disconnect without refreshing', async () => {
  const invoke = vi.fn((command: string) => Promise.resolve(command === 'bluetooth_status' ? 'open' : command === 'loopback_status' ? { running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null } : undefined))
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.text()).toContain('System playback connection is open.')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_disconnect')
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('ignores an old status reply after the user disconnects', async () => {
  let finishPoll!: (state: string) => void
  let statusCalls = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_status') {
      statusCalls++
      return statusCalls === 1 ? Promise.resolve('open') : new Promise<string>(resolve => { finishPoll = resolve })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  finishPoll('idle')
  await flushPromises()
  expect(wrapper.text()).toContain('Disconnected.')
  expect(wrapper.text()).not.toContain('Windows closed the playback connection.')
  wrapper.unmount()
})

it('waits for the phone to initiate a connection and can cancel waiting', async () => {
  let phase = 'idle'
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 4, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'bluetooth_disconnect') phase = 'idle'
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('select').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait for phone to connect')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 4, index: 0 })
  expect(wrapper.text()).toContain('Waiting for the phone to connect to Phone')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('starts output monitoring before waiting for a phone connection', async () => {
  let waiting = false
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(waiting ? 'waiting' : 'idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 5, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: monitoring ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_prepare') waiting = true
    if (command === 'bluetooth_disconnect') waiting = false
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  const calls = invoke.mock.calls.map(([command]) => command)
  expect(calls.indexOf('loopback_start')).toBeLessThan(calls.indexOf('bluetooth_prepare'))
  expect(wrapper.text()).toContain('Windows output monitoring is active.')
  expect(wrapper.text()).toContain('Waiting for Windows audio samples.')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Disconnected.')
  expect(wrapper.text()).not.toContain('Waiting for Windows audio samples.')
  wrapper.unmount()
})

it('falls back to phone-initiated wait when direct connect fails with monitoring enabled', async () => {
  let phase = 'idle'
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 10, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: monitoring ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_connect') return Promise.reject(new Error('Windows could not open the connection.'))
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'bluetooth_disconnect') phase = 'idle'
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Connect + monitor (auto wait fallback)')!.trigger('click')
  await flushPromises()
  const calls = invoke.mock.calls.map(([command]) => command)
  expect(calls.indexOf('loopback_start')).toBeLessThan(calls.indexOf('bluetooth_connect'))
  expect(calls.indexOf('bluetooth_connect')).toBeLessThan(calls.indexOf('bluetooth_prepare'))
  expect(wrapper.text()).toContain('Direct connection failed (Error: Windows could not open the connection.). Waiting for the phone to connect to Phone')
  expect(wrapper.text()).toContain('Windows output monitoring is active.')
  expect(wrapper.text()).toContain('Windows could not open the connection.')
  expect(wrapper.find('.command-error').exists()).toBe(false)
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  wrapper.unmount()
})

it('falls back to phone wait without requiring an output endpoint', async () => {
  let phase = 'idle'
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 12, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([])
    if (command === 'bluetooth_connect') return Promise.reject(new Error('Direct open failed.'))
    if (command === 'bluetooth_prepare') phase = 'waiting'
    return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  const action = wrapper.findAll('button').find(button => button.text() === 'Connect or wait for phone')!
  expect(action.attributes('disabled')).toBeUndefined()
  await action.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_connect', { generation: 12, index: 0 })
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 12, index: 0 })
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', expect.anything())
  expect(wrapper.text()).toContain('Waiting for the phone to connect to Phone through Bluetooth.')
  expect(wrapper.text()).not.toContain('Windows output monitoring is active.')
  wrapper.unmount()
})

it('stops newly started monitoring and reports both errors when automatic wait also fails', async () => {
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 11, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: monitoring ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_connect') return Promise.reject(new Error('Direct open failed.'))
    if (command === 'bluetooth_prepare') return Promise.reject(new Error('Phone wait failed.'))
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Connect + monitor (auto wait fallback)')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_connect', { generation: 11, index: 0 })
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 11, index: 0 })
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Direct connection failed: Error: Direct open failed. Phone-initiated wait also failed: Error: Phone wait failed.')
  expect(wrapper.find('.command-error').exists()).toBe(true)
  expect(wrapper.text()).not.toContain('Try waiting for the phone to connect')
  expect(wrapper.text()).not.toContain('Waiting for Windows audio samples.')
  expect(wrapper.findAll('button').find(button => button.text() === 'Start PCM check')).toBeTruthy()
  wrapper.unmount()
})

it('re-enters phone wait and keeps combined monitoring when Windows closes an opened connection', async () => {
  vi.useFakeTimers()
  let phase = 'idle'
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 8, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: monitoring ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  phase = 'open'
  await vi.advanceTimersByTimeAsync(1000)
  expect(wrapper.text()).toContain('System playback connection opened for Phone.')
  phase = 'idle'
  await vi.advanceTimersByTimeAsync(1000)
  await flushPromises()
  const prepareCalls = invoke.mock.calls.filter(([command]) => command === 'bluetooth_prepare')
  expect(prepareCalls).toHaveLength(2)
  expect(wrapper.text()).toContain('Windows closed the playback connection. Waiting for Phone to reconnect through Bluetooth.')
  expect(wrapper.text()).toContain('Windows output monitoring remains active.')
  expect(wrapper.text()).toContain('Waiting for Windows audio samples.')
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  wrapper.unmount()
})

it('restores combined wait and monitor ownership after reopening the panel', async () => {
  vi.useFakeTimers()
  let phase = 'idle'
  let monitoring = false
  let prepareCalls = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 21, names: ['Phone'], ids: ['phone-id'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_prepare') {
      prepareCalls++
      phase = 'waiting'
    }
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'phone-id')
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const first = mount(BluetoothProbe)
  await flushPromises()
  await first.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBe('true')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.recoveryPending')).toBe('true')
  phase = 'idle'
  first.unmount()

  const reopened = mount(BluetoothProbe)
  await flushPromises()
  expect(reopened.text()).toContain('Windows closed the playback connection. Waiting for Phone to reconnect through Bluetooth.')
  expect(prepareCalls).toBe(2)
  expect(reopened.text()).toContain('Windows output monitoring remains active.')
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')

  await reopened.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.recoveryPending')).toBeNull()
  reopened.unmount()
})

it('retries pending Bluetooth wait after the initial status query fails', async () => {
  let statusCalls = 0
  let phase = 'idle'
  let monitoring = true
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') {
      statusCalls++
      return statusCalls === 1
        ? Promise.reject(new Error('Temporary status failure.'))
        : Promise.resolve(phase)
    }
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 23, names: ['Phone'], ids: ['phone-id'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'phone-id')
  sessionStorage.setItem('sonimesh.bluetoothProbe.combinedMonitorOwned', 'true')
  sessionStorage.setItem('sonimesh.bluetoothProbe.recoveryPending', 'true')
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_prepare', { generation: 23, index: 0 })
  expect(wrapper.text()).toContain('Waiting for Phone to reconnect through Bluetooth.')
  expect(wrapper.text()).not.toContain('Temporary status failure.')
  expect(statusCalls).toBe(2)
  wrapper.unmount()
})

it('stops an owned monitor when its status cannot be read and Bluetooth wait fails', async () => {
  let monitoring = true
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 24, names: ['Phone'], ids: ['phone-id'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.reject(new Error('Monitor status unavailable.'))
    if (command === 'bluetooth_prepare') return Promise.reject(new Error('Phone wait failed.'))
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'phone-id')
  sessionStorage.setItem('sonimesh.bluetoothProbe.combinedMonitorOwned', 'true')
  sessionStorage.setItem('sonimesh.bluetoothProbe.recoveryPending', 'true')
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(monitoring).toBe(false)
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  expect(wrapper.text()).toContain('waiting could not restart: Error: Phone wait failed.')
  wrapper.unmount()
})

it('stops a restored combined monitor when phone wait cannot resume', async () => {
  vi.useFakeTimers()
  let phase = 'idle'
  let monitoring = false
  let prepareCalls = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 22, names: ['Phone'], ids: ['phone-id'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_prepare') {
      prepareCalls++
      if (prepareCalls === 1) {
        phase = 'waiting'
        return Promise.resolve()
      }
      return Promise.reject(new Error('Phone wait failed after reopening.'))
    }
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  sessionStorage.setItem('sonimesh.bluetoothProbe.sourceId', 'phone-id')
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }

  const first = mount(BluetoothProbe)
  await flushPromises()
  await first.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  phase = 'open'
  first.unmount()

  const reopened = mount(BluetoothProbe)
  await flushPromises()
  phase = 'idle'
  await vi.advanceTimersByTimeAsync(1000)
  await flushPromises()
  expect(prepareCalls).toBe(2)
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(reopened.text()).toContain('waiting could not restart: Error: Phone wait failed after reopening.')
  expect(sessionStorage.getItem('sonimesh.bluetoothProbe.combinedMonitorOwned')).toBeNull()
  reopened.unmount()
})

it('stops combined monitoring and reports an error when phone wait cannot resume', async () => {
  vi.useFakeTimers()
  let phase = 'idle'
  let prepareCalls = 0
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 18, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: 2, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_prepare') {
      prepareCalls++
      if (prepareCalls === 1) {
        phase = 'waiting'
        return Promise.resolve()
      }
      return Promise.reject(new Error('The source is no longer available.'))
    }
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  phase = 'open'
  await vi.advanceTimersByTimeAsync(1000)
  phase = 'idle'
  await vi.advanceTimersByTimeAsync(1000)
  await flushPromises()
  expect(prepareCalls).toBe(2)
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('waiting could not restart: Error: The source is no longer available.')
  expect(wrapper.text()).not.toContain('Disconnect Bluetooth')
  wrapper.unmount()
})

it('keeps a pre-existing output monitor running after Bluetooth disconnects', async () => {
  let phase = 'idle'
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 9, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 10, peak: 0, error: null })
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'bluetooth_disconnect') phase = 'idle'
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', expect.anything())
  expect(invoke).not.toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('stops monitoring if the combined wait operation cannot prepare Bluetooth', async () => {
  let monitoring = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 6, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: monitoring, endpointName: 'Default speakers', sampleRate: monitoring ? 48000 : 0, channels: monitoring ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') {
      monitoring = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    if (command === 'bluetooth_prepare') return Promise.reject(new Error('Could not prepare Bluetooth.'))
    if (command === 'loopback_stop') {
      monitoring = false
      return Promise.resolve({ running: false, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Could not prepare Bluetooth.')
  expect(wrapper.text()).not.toContain('Waiting for Windows audio samples.')
  wrapper.unmount()
})

it('does not prepare Bluetooth when output monitoring fails to start', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 7, names: ['Phone'] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') return Promise.reject(new Error('Output device is unavailable.'))
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  await wrapper.get('#bluetooth-source').setValue('0')
  await wrapper.findAll('button').find(button => button.text() === 'Wait + monitor system output')!.trigger('click')
  await flushPromises()
  expect(invoke).not.toHaveBeenCalledWith('bluetooth_prepare', expect.anything())
  expect(wrapper.text()).toContain('Output device is unavailable.')
  expect(wrapper.findAll('.command-error')).toHaveLength(1)
  wrapper.unmount()
})

it('starts and stops the Windows system-output PCM check', async () => {
  let running = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([
      { id: 'default-endpoint', name: 'Default speakers', isDefault: true },
      { id: 'bt-endpoint', name: 'Bluetooth headphones', isDefault: false },
    ])
    if (command === 'loopback_status') return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: running ? 48000 : 0, channels: running ? 2 : 0, samples: 4096, peak: 0.5, error: null })
    if (command === 'loopback_start') { running = true; return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null }) }
    if (command === 'loopback_stop') { running = false; return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 4096, peak: 0.5, error: null }) }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('#loopback-endpoint').setValue('bt-endpoint')
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'bt-endpoint' })
  expect(wrapper.text()).toContain('48000 Hz, 2 channels')
  expect(wrapper.text()).toContain('Default speakers')
  expect(wrapper.get('.pcm-level').attributes('aria-valuenow')).toBe('50.0')
  expect(wrapper.get('.pcm-level span').attributes('style')).toContain('width: 50%')
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('4096 samples; recent peak 50.0%;')
  expect(wrapper.text()).toContain('PCM activity detected recently.')
  wrapper.unmount()
})

it('continues monitoring after a failed stop and allows another stop', async () => {
  vi.useFakeTimers()
  let running = true
  let samples = 0
  let stopCalls = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'output', name: 'Speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running, endpointId: 'output', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples, peak: samples ? 0.04 : 0, error: null })
    if (command === 'loopback_stop') {
      if (++stopCalls === 1) return Promise.reject(new Error('Stop failed'))
      running = false
      return Promise.resolve({ running, endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples, peak: 0.04, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await vi.advanceTimersByTimeAsync(5000)
  expect(wrapper.text()).toContain('No Windows audio samples have arrived yet.')
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Stop failed')
  expect(wrapper.text()).toContain('No Windows audio samples have arrived yet.')
  samples = 4096
  await vi.advanceTimersByTimeAsync(1000)
  expect(wrapper.text()).toContain('4096 samples; recent peak 4.0%')
  expect(wrapper.text()).not.toContain('No Windows audio samples have arrived yet.')
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  expect(stopCalls).toBe(2)
  expect(wrapper.text()).toContain('Start PCM check')
  wrapper.unmount()
})

it('shows output endpoint format when Windows provides it', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([
      { id: 'bt-endpoint', name: 'Bluetooth headphones', isDefault: true, sampleRate: 44100, channels: 2 },
      { id: 'unknown-endpoint', name: 'Unavailable format', isDefault: false, sampleRate: null, channels: null },
    ])
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  const options = wrapper.findAll('#loopback-endpoint option').map(option => option.text())
  expect(options).toContain('Bluetooth headphones (default, 44.1 kHz, 2 ch)')
  expect(options).toContain('Unavailable format')
  wrapper.unmount()
})

it('distinguishes same-name outputs across refreshes and starts the chosen endpoint', async () => {
  const endpoints = [
    { id: 'output-b', name: 'Speakers', isDefault: true, sampleRate: 48000, channels: 2 },
    { id: 'output-a', name: 'Speakers', isDefault: false, sampleRate: 48000, channels: 2 },
  ]
  let refreshes = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_endpoints') return Promise.resolve(++refreshes === 1 ? endpoints : [...endpoints].reverse())
    if (command === 'loopback_start') return Promise.resolve({ running: true, endpointId: 'output-a', endpointName: 'Speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.get('option[value="output-a"]').text()).toBe('Speakers (1) (48 kHz, 2 ch)')
  expect(wrapper.get('option[value="output-b"]').text()).toBe('Speakers (2) (default, 48 kHz, 2 ch)')
  await wrapper.get('#loopback-endpoint').setValue('output-a')
  await wrapper.findAll('button').find(button => button.text() === 'Refresh output devices')!.trigger('click')
  await flushPromises()
  expect(wrapper.get('option[value="output-a"]').text()).toBe('Speakers (1) (48 kHz, 2 ch)')
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('output-a')
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'output-a' })
  wrapper.unmount()
})

it('restarts a failed PCM monitor on the refreshed default endpoint', async () => {
  let endpointRefreshes = 0
  let recovered = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: [] })
    if (command === 'loopback_status') return Promise.resolve(recovered
      ? { running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 128, peak: 0, runPeak: 0, error: null }
      : { running: true, endpointName: 'Removed output', sampleRate: 48000, channels: 2, samples: 4096, peak: 0, runPeak: 0.1, error: 'The Windows loopback stream reported an error.' })
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve(endpointRefreshes === 1
        ? [{ id: 'removed-output', name: 'Removed output', isDefault: false }]
        : [{ id: 'default-output', name: 'Default speakers', isDefault: true }])
    }
    if (command === 'loopback_stop') return Promise.resolve({ running: false, endpointName: 'Removed output', sampleRate: 48000, channels: 2, samples: 4096, peak: 0, runPeak: 0.1, error: 'The Windows loopback stream reported an error.' })
    if (command === 'loopback_start') {
      recovered = true
      return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, runPeak: 0, error: null })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Recover PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(invoke).toHaveBeenCalledWith('loopback_start', { endpointId: 'default-output' })
  expect((wrapper.get('#loopback-endpoint').element as HTMLSelectElement).value).toBe('default-output')
  expect(wrapper.text()).toContain('Default speakers')
  expect(wrapper.text()).toContain('Removed output is unavailable. Using Default speakers.')
  expect(wrapper.findAll('.command-error')).toHaveLength(0)
  wrapper.unmount()
})

it('keeps the monitor error visible when recovery finds no output endpoint', async () => {
  let endpointRefreshes = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointName: 'Removed output', sampleRate: 48000, channels: 2, samples: 10, peak: 0, runPeak: 0, error: 'The Windows loopback stream reported an error.' })
    if (command === 'loopback_endpoints') {
      endpointRefreshes++
      return Promise.resolve(endpointRefreshes === 1 ? [{ id: 'removed-output', name: 'Removed output', isDefault: true }] : [])
    }
    if (command === 'loopback_stop') return Promise.resolve({ running: false, endpointName: 'Removed output', sampleRate: 48000, channels: 2, samples: 10, peak: 0, runPeak: 0, error: 'The Windows loopback stream reported an error.' })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Recover PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).not.toHaveBeenCalledWith('loopback_start', expect.anything())
  expect(wrapper.text()).toContain('No Windows output endpoints are available.')
  expect(wrapper.findAll('button').find(button => button.text() === 'Recover PCM check')).toBeTruthy()
  wrapper.unmount()
})

it('retains the monitor-session peak after recent activity decays', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 4096, peak: 0, runPeak: 0.5, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.text()).toContain('recent peak 0.0%; session peak 50.0%')
  expect(wrapper.text()).toContain('No recent PCM activity.')
  wrapper.unmount()
})

it('plays a short tone on the monitored endpoint and refreshes its meter', async () => {
  let running = false
  let samples = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: running ? 48000 : 0, channels: running ? 2 : 0, samples, peak: samples ? 0.04 : 0, error: null })
    if (command === 'loopback_start') { running = true; return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples, peak: 0, error: null }) }
    if (command === 'loopback_test_tone') { samples = 192000; return Promise.resolve() }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Play test tone')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_test_tone', { endpointId: 'default-endpoint' })
  expect(wrapper.text()).toContain('Test tone finished.')
  expect(wrapper.text()).toContain('192000 samples; recent peak 4.0%')
  wrapper.unmount()
})

it('keeps a single meter polling loop after repeated test tones', async () => {
  vi.useFakeTimers()
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointId: 'default-endpoint', endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 10, peak: 0, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  for (let count = 0; count < 2; count++) {
    await wrapper.findAll('button').find(button => button.text() === 'Play test tone')!.trigger('click')
    await flushPromises()
  }
  const statusCalls = () => invoke.mock.calls.filter(([command]) => command === 'loopback_status').length
  const before = statusCalls()
  await vi.advanceTimersByTimeAsync(2000)
  expect(statusCalls() - before).toBe(2)
  wrapper.unmount()
})

it('keeps the output monitor running until its test tone finishes', async () => {
  let finishTone!: () => void
  let running = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: running ? 48000 : 0, channels: running ? 2 : 0, samples: 0, peak: 0, error: null })
    if (command === 'loopback_start') { running = true; return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null }) }
    if (command === 'loopback_test_tone') return new Promise<void>(resolve => { finishTone = resolve })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Play test tone')!.trigger('click')
  await flushPromises()
  const stopButton = wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!
  expect(stopButton.attributes('disabled')).toBeDefined()
  finishTone()
  await flushPromises()
  expect(stopButton.attributes('disabled')).toBeUndefined()
  wrapper.unmount()
})

it('distinguishes an open PCM stream with no recent signal', async () => {
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running: true, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 4096, peak: 0, error: null })
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  expect(wrapper.text()).toContain('No recent PCM activity.')
  expect(wrapper.text()).toContain('Measures the selected Windows playback mix')
  wrapper.unmount()
})

it('shows when a running loopback stream has not delivered samples yet', async () => {
  vi.useFakeTimers()
  let running = false
  let samples = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_endpoints') return Promise.resolve([{ id: 'default-endpoint', name: 'Default speakers', isDefault: true }])
    if (command === 'loopback_status') return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: running ? 48000 : 0, channels: running ? 2 : 0, samples, peak: 0, error: null })
    if (command === 'loopback_start') { running = true; return Promise.resolve({ running, endpointName: 'Default speakers', sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null }) }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Waiting for Windows audio samples.')
  await vi.advanceTimersByTimeAsync(5000)
  await flushPromises()
  expect(wrapper.text()).toContain('No Windows audio samples have arrived yet. Start playback and check the selected output endpoint.')
  samples = 4096
  await vi.advanceTimersByTimeAsync(1000)
  await flushPromises()
  expect(wrapper.text()).not.toContain('No Windows audio samples have arrived yet.')
  expect(wrapper.text()).toContain('No recent PCM activity.')
  wrapper.unmount()
})
