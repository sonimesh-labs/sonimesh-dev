import { afterEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import BluetoothProbe from './BluetoothProbe.vue'

afterEach(() => { delete window.__TAURI__; vi.useRealTimers() })

it('refreshes candidates and connects and disconnects the selected source', async () => {
  let connected = false
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 3, names: ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(connected)
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
  expect(wrapper.text()).toContain('System playback connection opened.')
  await wrapper.findAll('button')[1].trigger('click')
  await flushPromises()
  expect(invoke).toHaveBeenCalledWith('bluetooth_disconnect')
  expect(wrapper.text()).toContain('Disconnected.')
  wrapper.unmount()
})

it('reports empty discovery and system closure', async () => {
  let empty = true
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_candidates') return Promise.resolve({ generation: 1, names: empty ? [] : ['Phone'] })
    if (command === 'bluetooth_status') return Promise.resolve(false)
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
  const invoke = vi.fn((command: string) => Promise.resolve(command === 'bluetooth_status'))
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
  let finishPoll!: (open: boolean) => void
  let statusCalls = 0
  const invoke = vi.fn((command: string) => {
    if (command === 'bluetooth_status') {
      statusCalls++
      return statusCalls === 1 ? Promise.resolve(true) : new Promise<boolean>(resolve => { finishPoll = resolve })
    }
    return Promise.resolve()
  })
  window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
  const wrapper = mount(BluetoothProbe)
  await flushPromises()
  await wrapper.findAll('button').find(button => button.text() === 'Disconnect Bluetooth')!.trigger('click')
  await flushPromises()
  finishPoll(false)
  await flushPromises()
  expect(wrapper.text()).toContain('Disconnected.')
  expect(wrapper.text()).not.toContain('Windows closed the playback connection.')
  wrapper.unmount()
})
