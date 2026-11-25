import { afterEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import BluetoothProbe from './BluetoothProbe.vue'

afterEach(() => { delete window.__TAURI__; vi.useRealTimers() })

it('refreshes candidates and connects and disconnects the selected source', async () => {
  let connected = false
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 3, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(connected ? 'open' : 'idle')
    if (command === 'bluetooth_connect') connected = true
    if (command === 'bluetooth_disconnect') connected = false
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('button').trigger('click')
  await flushPromises()
  await wrapper.get('select').setValue('0')
  await wrapper.findAll('button')[1].trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_connect', { generation: 3, index: 0 })
  expect(wrapper.text()).toContain('System playback connection opened for Phone.')
  await wrapper.findAll('button')[1].trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_disconnect')
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('reports empty discovery and system closure', async () => {
  let empty = true
  const invoke = vi.fn((command: string) => {
    if (command === 'loopback_status') return Promise.resolve({ running: false, sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: empty ? [] : ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('button').trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('No playback sources found.')
  empty = false
  await wrapper.get('button').trigger('click')
  await flushPromises()
  await wrapper.get('select').setValue('0')
  await wrapper.findAll('button')[1].trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('Windows closed the playback connection.')
  wrapper.unmount()
})

it('restores an existing connection and allows disconnect without refreshing', async () => {
  const invoke = vi.fn((command: string) => Promise.resolve(command === 'bluetooth_status' ? 'open' : command === 'loopback_status' ? { running: false, sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null } : undefined))
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
    if (command === 'loopback_status') return Promise.resolve({ running: false, sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
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
    if (command === 'loopback_status') return Promise.resolve({ running: false, sampleRate: 0, channels: 0, samples: 0, peak: 0, error: null })
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 4, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(phase)
    if (command === 'bluetooth_prepare') phase = 'waiting'
    if (command === 'bluetooth_disconnect') phase = 'idle'
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.get('button').trigger('click')
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

it('starts and stops the Windows system-output PCM check', async () => {
  let running = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') return Promise.resolve('idle')
    if (command === 'loopback_status') return Promise.resolve({ running, sampleRate: running ? 48000 : 0, channels: running ? 2 : 0, samples: 4096, peak: 0.5, error: null })
    if (command === 'loopback_start') { running = true; return Promise.resolve({ running, sampleRate: 48000, channels: 2, samples: 0, peak: 0, error: null }) }
    if (command === 'loopback_stop') { running = false; return Promise.resolve({ running, sampleRate: 48000, channels: 2, samples: 4096, peak: 0.5, error: null }) }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Start PCM check')!.trigger('click')
  await flushPromises()
  expect(wrapper.text()).toContain('48000 Hz, 2 channels')
  await wrapper.findAll('button').find(button => button.text() === 'Stop PCM check')!.trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('loopback_stop')
  expect(wrapper.text()).toContain('4096 samples; recent peak 50.0%.')
  wrapper.unmount()
})
