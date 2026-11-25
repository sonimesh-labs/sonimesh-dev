import { afterEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import BluetoothProbe from './BluetoothProbe.vue'

afterEach(() => { delete window.__TAURI__; vi.useRealTimers() })

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

it('preserves the selected source by name when discovery order changes', async () => {
  let generation = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running: false, endpointName: '', sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') {
      generation++
      return Promise.resolve({ generation, names: generation === 1 ? ['Phone', 'Headset'] : ['Headset', 'Phone'] })
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
  await refreshButton.trigger('click')
  await flushPromises()
  expect((wrapper.get('#bluetooth-source').element as HTMLSelectElement).value).toBe('0')
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
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: empty ? [] : ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Refresh Bluetooth sources')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('No playback sources found.')
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

it('stops combined output monitoring when Windows closes an opened connection', async () => {
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
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('Windows closed the playback connection.')
  expect(wrapper.text()).not.toContain('Waiting for Windows audio samples.')
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
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('4096 samples; recent peak 50.0%;')
  expect(wrapper.text()).toContain('PCM activity detected recently.')
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
