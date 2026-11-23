import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import OutputProbe from './OutputProbe.vue'

const originalClipboard = navigator.clipboard
const originalShare = navigator.share

afterEach(() => {
  vi.useRealTimers()
  delete window.__TAURI__
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: originalClipboard })
  Object.defineProperty(navigator, 'share', { configurable: true, value: originalShare })
})

function mountStoppedProbe() {
  const core = window.__TAURI__!.core
  const invoke = core.invoke
  let initialStatus = true
  core.invoke = ((command: string, args?: Record<string, unknown>) => {
    if (command === 'lab_status' && initialStatus) {
      initialStatus = false
      return Promise.resolve({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
    }
    return args === undefined ? invoke(command) : invoke(command, args)
  }) as typeof invoke
  return mount(OutputProbe)
}

describe('desktop output probe', () => {
  it('shows the host error if a listener starts before a test sound request', async () => {
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [{ label: 'Speakers', sampleRateHz: 48000, channels: 2, isDefault: true }] })
      if (command === 'play_test_tone') return Promise.reject('Stop the listener before testing an output.')
      return Promise.resolve({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toBe('Stop the listener before testing an output.')
    wrapper.unmount()
  })

  it('waits for a test sound to finish before starting the listener, including after a playback failure', async () => {
    let rejectTone!: (reason: string) => void
    const tone = new Promise((_resolve, reject) => { rejectTone = reject })
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [{ label: 'Speakers', sampleRateHz: 48000, channels: 2, isDefault: true }] })
      if (command === 'play_test_tone') return tone
      return Promise.resolve({ running: true, status: 'Starting', invitation: null, metrics: null, volume: null, muted: null })
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    const start = wrapper.findAll('button').find(button => button.text() === 'Start listener')!
    expect(start.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Wait for the test sound')
    await start.trigger('click')
    expect(invoke).not.toHaveBeenCalledWith('start_lab', expect.anything())
    rejectTone('Could not start this output.')
    await flushPromises()
    expect(wrapper.text()).toContain('Could not start this output.')
    expect(start.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('disables test sounds while the listener starts and runs, then restores them after stop', async () => {
    let resolveStart!: (snapshot: unknown) => void
    const pendingStart = new Promise(resolve => { resolveStart = resolve })
    const stopped = { running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null }
    const running = { ...stopped, running: true, status: 'Waiting' }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [{ label: 'Speakers', sampleRateHz: 48000, channels: 2, isDefault: true }] })
      if (command === 'start_lab') return pendingStart
      if (command === 'stop_lab') return Promise.resolve(stopped)
      return Promise.resolve(running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    const tone = wrapper.get('[aria-label="Play test tone on Speakers"]')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(tone.attributes('disabled')).toBeDefined()
    await tone.trigger('click')
    expect(invoke).not.toHaveBeenCalledWith('play_test_tone', expect.anything())
    resolveStart(running)
    await flushPromises()
    expect(tone.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Stop the listener before testing an output')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(tone.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('restores test sounds when listener startup fails', async () => {
    const invoke = vi.fn()
      .mockResolvedValueOnce({ generation: 1, devices: [{ label: 'Speakers', sampleRateHz: 48000, channels: 2, isDefault: true }] })
      .mockRejectedValueOnce('Could not launch the lab listener.')
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Could not launch the lab listener.')
    expect(wrapper.get('[aria-label="Play test tone on Speakers"]').attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('reports an unavailable desktop bridge while checking the listener', async () => {
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Could not check the listener')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('restores a running listener and stops it before inspecting outputs', async () => {
    vi.useFakeTimers()
    const invoke = vi.fn()
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', metrics: null, volume: 60, muted: false })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting for Android', invitation: '01ab', metrics: null, volume: 60, muted: false })
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Session active')
    expect(wrapper.text()).toContain('running listener may use different settings')
    expect(invoke).not.toHaveBeenCalledWith('probe_outputs', expect.anything())
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Waiting for Android')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('stop_lab')
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 0, selectedIndex: null })
    wrapper.unmount()
  })

  it('retries an initial status failure before enabling listener controls', async () => {
    vi.useFakeTimers()
    const invoke = vi.fn().mockRejectedValueOnce(new Error('temporary'))
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Retrying')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Stopped')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('loads outputs if a restored listener exits on its own', async () => {
    vi.useFakeTimers()
    const invoke = vi.fn()
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: 100, muted: false })
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Listener exited', invitation: null, metrics: null, volume: null, muted: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Listener exited')
    expect(wrapper.text()).not.toContain('running listener may use different settings')
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 0, selectedIndex: null })
    wrapper.unmount()
  })

  it('keeps the last session observations visible after the listener exits', async () => {
    vi.useFakeTimers()
    const metrics = { packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2', decodeErrors: '1', buffered: '4', output: 'active', late: '0', resyncDropped: '0', playout: 'fixed', adaptive: null }
    const invoke = vi.fn()
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', events: ['READY', 'RESULT receiver started'], metrics, volume: 50, muted: false })
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Listener exited', invitation: null, events: ['READY', 'RESULT receiver started', 'OUTPUT stopped'], metrics, volume: null, muted: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Listener exited')
    expect(wrapper.text()).toContain('Last session observations')
    expect(wrapper.text()).toContain('115')
    expect(wrapper.text()).toContain('Last available update')
    expect(wrapper.text()).toContain('Last reported: Active')
    expect(wrapper.text()).toContain('RESULT receiver started')
    expect(wrapper.text()).toContain('OUTPUT stopped')
    expect(wrapper.find('#lab-invitation').exists()).toBe(false)
    wrapper.unmount()
  })

  it('ignores an old status response after stopping a restored listener', async () => {
    vi.useFakeTimers()
    let resolvePoll!: (snapshot: unknown) => void
    const pendingPoll = new Promise(resolve => { resolvePoll = resolve })
    const invoke = vi.fn()
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: 100, muted: false })
      .mockReturnValueOnce(pendingPoll)
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    resolvePoll({ running: true, sessionActive: true, status: 'Stale session', invitation: null, metrics: null, volume: 100, muted: false })
    await flushPromises()
    expect(wrapper.text()).toContain('Stopped')
    expect(wrapper.text()).not.toContain('Stale session')
    wrapper.unmount()
  })
  it('suggests a route address only when requested and applies it explicitly', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce('192.168.1.10')
      .mockResolvedValueOnce({ running: true, status: 'Starting', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#android-host').setValue(' 192.168.1.20 ')
    await wrapper.findAll('button').find(button => button.text() === 'Suggest desktop address')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('suggest_lab_host', { androidHost: '192.168.1.20' })
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
    await wrapper.findAll('button').find(button => button.text() === 'Use address')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '192.168.1.10', generation: 1, index: null, startupBufferMs: 60, adaptive: false })
    wrapper.unmount()
  })

  it('shows route errors and preserves the manually entered address', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockRejectedValueOnce('No network route to that address. Enter the desktop address manually.')
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-host').setValue('10.0.2.2')
    await wrapper.findAll('button').find(button => button.text() === 'Suggest desktop address')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('No network route')
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('10.0.2.2')
    wrapper.unmount()
  })
  it('sends audio controls and displays the listener-confirmed values', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Waiting', invitation: null, metrics: null, volume: 100, muted: false })
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({ running: true, status: 'Waiting', invitation: null, metrics: null, volume: 35, muted: false })
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({ running: true, status: 'Waiting', invitation: null, metrics: null, volume: 35, muted: true })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.get('#lab-volume').setValue('35')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('control_lab', { volume: 35, muted: null })
    expect(wrapper.text()).toContain('Receiver volume: 100%')
    expect((wrapper.get('#lab-volume').element as HTMLInputElement).value).toBe('100')
    expect(wrapper.get('#lab-volume').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('Waiting for the listener to confirm')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Receiver volume: 35%')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('control_lab', { volume: null, muted: true })
    expect(wrapper.findAll('button').find(button => button.text() === 'Mute')!.attributes('disabled')).toBeDefined()
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Receiver volume: 35%')
    expect((wrapper.get('#lab-volume').element as HTMLInputElement).value).toBe('35')
    expect(wrapper.findAll('button').some(button => button.text() === 'Unmute')).toBe(true)
    wrapper.unmount()
  })

  it('allows retry after an unconfirmed adjustment without inventing a confirmed value', async () => {
    vi.useFakeTimers()
    const running = { running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: 100, muted: false }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'control_lab') return Promise.resolve(undefined)
      return Promise.resolve(running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    await flushPromises()
    await vi.advanceTimersByTimeAsync(5000)
    expect(wrapper.text()).toContain('Listener did not confirm the adjustment')
    expect(wrapper.findAll('button').find(button => button.text() === 'Mute')!.attributes('disabled')).toBeUndefined()
    expect(wrapper.get('#lab-volume').element).toHaveProperty('value', '100')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    expect(invoke.mock.calls.filter(([command]) => command === 'control_lab')).toHaveLength(2)
    wrapper.unmount()
  })

  it('shows a control send failure and immediately permits retry', async () => {
    const running = { running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: 100, muted: false }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'control_lab') return Promise.reject('Could not send audio control.')
      return Promise.resolve(running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Could not send audio control.')
    expect(wrapper.findAll('button').find(button => button.text() === 'Mute')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('ignores a pending control response after stopping and restarting the listener', async () => {
    let rejectControl!: (cause: string) => void
    const pending = new Promise((_resolve, reject) => { rejectControl = reject })
    let resolveStop!: (snapshot: unknown) => void
    const pendingStop = new Promise(resolve => { resolveStop = resolve })
    const stopped = { running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null }
    const running = { ...stopped, running: true, status: 'Waiting', volume: 100, muted: false }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'control_lab') return pending
      if (command === 'stop_lab') return pendingStop
      return Promise.resolve(running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    expect(wrapper.findAll('button').find(button => button.text() === 'Mute')!.attributes('disabled')).toBeDefined()
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    expect(invoke.mock.calls.filter(([command]) => command === 'control_lab')).toHaveLength(1)
    resolveStop(stopped)
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    rejectControl('Old listener failed')
    await flushPromises()
    expect(wrapper.text()).not.toContain('Old listener failed')
    expect(wrapper.findAll('button').find(button => button.text() === 'Mute')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })
  it('requires a new selection if the selected device disappears on refresh', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 4, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ] }).mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: false, status: 'Stopped', invitation: null, metrics: null })
      .mockResolvedValueOnce({ generation: 5, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('1')
    await wrapper.get('#lab-buffer').setValue('40')
    await wrapper.get('#lab-playout').setValue(true)
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '127.0.0.1', generation: 4, index: 1, startupBufferMs: 40, adaptive: true })
    expect(wrapper.get('#lab-buffer').attributes('disabled')).toBeDefined()
    expect(wrapper.get('#lab-output').attributes('disabled')).toBeDefined()
    expect(wrapper.get('button').attributes('disabled')).toBeDefined()
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 4, selectedIndex: 1 })
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    await wrapper.get('#lab-output').setValue('')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })
  it('preserves a selected endpoint when its index changes on refresh', async () => {
    const speakers = { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true }
    const headphones = { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 4, devices: [speakers, headphones], selectedIndex: null })
      .mockResolvedValueOnce({ generation: 5, devices: [headphones, speakers], selectedIndex: 0 })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('1')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 4, selectedIndex: 1 })
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('0')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '127.0.0.1', generation: 5, index: 0, startupBufferMs: 60, adaptive: false })
    wrapper.unmount()
  })
  it.each(['format unavailable', 'refresh failed'])('requires reselection when %s', async (failure) => {
    const device = { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [device], selectedIndex: null })
    if (failure === 'refresh failed') invoke.mockRejectedValueOnce(new Error('unavailable'))
    else invoke.mockResolvedValueOnce({ generation: 2, devices: [{ ...device, sampleRateHz: null, channels: null }], selectedIndex: 0 })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('0')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    const start = wrapper.findAll('button').find(button => button.text() === 'Start listener')!
    expect(start.attributes('disabled')).toBeDefined()
    await start.trigger('click')
    expect(invoke).not.toHaveBeenCalledWith('start_lab', expect.anything())
    wrapper.unmount()
  })
  it('starts and stops the lab listener with the specified control address', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: false, status: 'Stopped', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-host').setValue('10.0.2.2')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '10.0.2.2', generation: 1, index: null, startupBufferMs: 60, adaptive: false })
    expect(wrapper.text()).toContain('Starting listener')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('stop_lab')
    expect(wrapper.text()).toContain('Stopped')
    wrapper.unmount()
  })

  it('shows listener launch errors and permits retry', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockRejectedValueOnce('Enter a valid IPv4 address.')
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    const start = wrapper.findAll('button').find(button => button.text() === 'Start listener')!
    await start.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Enter a valid IPv4 address.')
    await start.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Starting listener')
    wrapper.unmount()
  })

  it('displays the invitation returned by the running listener', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab' })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect((wrapper.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('01ab')
    expect(wrapper.get('.lab-invitation-qr').attributes('src')).toMatch(/^data:image\/svg\+xml,/)
    expect(wrapper.text()).toContain('Waiting for Android')
    wrapper.unmount()
  })

  it('copies the displayed invitation for Android setup', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab' })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Copy invitation')!.trigger('click')
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith('01ab')
    expect(wrapper.text()).toContain('Invitation copied.')
    wrapper.unmount()
  })

  it('shares the displayed invitation through the system share API', async () => {
    const share = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'share', { configurable: true, value: share })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab' })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Share invitation')!.trigger('click')
    await flushPromises()
    expect(share).toHaveBeenCalledWith({ title: 'SoniMesh lab invitation', text: '01ab' })
    expect(wrapper.text()).toContain('Invitation shared.')
    wrapper.unmount()
  })

  it('explains when invitation copying is unavailable', async () => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab' })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Copy invitation')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Could not copy the invitation')
    wrapper.unmount()
  })

  it('copies bounded session evidence without the invitation', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const metrics = {
      packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2',
      decodeErrors: '1', buffered: '4', output: 'active', media: 'receiving',
      late: '5', resyncDropped: '1', playout: 'adaptive',
      adaptive: { target: 3, min: 2, max: 4, jitterMs: 17, gated: '9', forced: '1', resets: '2' },
    }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: 'secret-invitation', metrics })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    await flushPromises()
    const copied = writeText.mock.calls[0][0] as string
    expect(copied).toContain('Accepted packets: 120')
    expect(copied).toContain('Output: active')
    expect(copied).toContain('Adaptive target: 3')
    expect(copied).toContain('Gated ticks: 9')
    expect(copied).not.toContain('secret-invitation')
    expect(wrapper.text()).toContain('Session evidence copied.')
    wrapper.unmount()
  })

  it('shows live and final session observations, then clears them on the next client', async () => {
    const metrics = { packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2', decodeErrors: '1', buffered: '4', output: 'recovering', late: '9007199254740993', resyncDropped: '4', media: 'receiving' }
    const finalMetrics = { ...metrics, decoded: '120', output: 'stopped', late: '9007199254740994', resyncDropped: '5', media: 'interrupted' }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', metrics })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Session ended; waiting for Android', invitation: '01ab', metrics: finalMetrics })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Android connecting', invitation: '01ab', metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.get('.lab-metrics').text()).toContain('Accepted packets120')
    expect(wrapper.get('.lab-metrics').text()).toContain('Media arrivalReceiving packets')
    expect(wrapper.get('.lab-metrics').text()).toContain('Late packets9007199254740993')
    expect(wrapper.get('.lab-metrics').text()).toContain('Frames dropped on resync4')
    expect(wrapper.get('.lab-metrics').text()).toContain('Decoded peak0.250')
    expect(wrapper.get('.lab-metrics').text()).toContain('OutputRecovering')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Last session observations')
    expect(wrapper.text()).toContain('Final update after the stop drain')
    expect(wrapper.get('.lab-metrics').text()).toContain('Decoded frames120')
    expect(wrapper.get('.lab-metrics').text()).toContain('Late packets9007199254740994')
    expect(wrapper.get('.lab-metrics').text()).toContain('Frames dropped on resync5')
    expect(wrapper.get('.lab-metrics').text()).toContain('OutputStopped')
    expect(wrapper.get('.lab-metrics').text()).toContain('Media arrivalLast reported: No recent packets')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.find('.lab-metrics').exists()).toBe(false)
    expect(wrapper.text()).toContain('Android connecting')
    wrapper.unmount()
  })

  it('shows the listener’s adaptive epoch and keeps the last update until the next session', async () => {
    const base = { packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2', decodeErrors: '1', buffered: '4', output: 'active', playout: 'adaptive', media: 'waiting' }
    const first = { ...base, adaptive: { target: 3, min: 2, max: 4, jitterMs: 17, gated: '9007199254740993', forced: '2', resets: '1' } }
    const reset = { ...base, output: 'stopped', adaptive: { target: 0, min: 0, max: 0, jitterMs: 0, gated: '0', forced: '0', resets: '2' } }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: null, metrics: first })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: reset })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Android connecting', invitation: null, metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.get('.lab-metrics').text()).toContain('Playout modeAdaptive (experimental)')
    expect(wrapper.get('.lab-metrics').text()).toContain('Media arrivalWaiting for first packet')
    expect(wrapper.get('.lab-metrics').text()).not.toContain('Late packets')
    expect(wrapper.get('.lab-metrics').text()).not.toContain('Frames dropped on resync')
    expect(wrapper.get('.lab-metrics').text()).toContain('Adaptive target3 frames (2–4)')
    expect(wrapper.get('.lab-metrics').text()).toContain('Gated ticks9007199254740993')
    expect(wrapper.text()).toContain('reset with the stream epoch')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.get('.lab-metrics').text()).toContain('Adaptive targetAwaiting packet for this epoch')
    expect(wrapper.get('.lab-metrics').text()).toContain('Stream resets2')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.find('.lab-metrics').exists()).toBe(false)
    wrapper.unmount()
  })

  it('labels a failed session’s last update as potentially incomplete', async () => {
    const metrics = { packets: '5', rejected: '0', decoded: '4', peak: '0.100', plc: '1', decodeErrors: '0', buffered: '0', output: 'active' }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Starting', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Session failed; waiting for Android', invitation: null, metrics })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Last available update; the session may have ended before its final report.')
    expect(wrapper.get('.lab-metrics').text()).toContain('OutputLast reported: Active')
    wrapper.unmount()
  })

  it('retries a failed status request while the listener is running', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab', metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Retrying')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Waiting for Android')
    expect(wrapper.text()).not.toContain('Retrying')
    wrapper.unmount()
  })

  it('reconciles listener state after a failed start, retrying an unavailable status', async () => {
    const running = { running: true, sessionActive: false, status: 'Waiting for Android', invitation: null, metrics: null, volume: 100, muted: false }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'start_lab') return Promise.reject('Start response lost.')
      if (command === 'lab_status') return invoke.mock.calls.filter(([name]) => name === 'lab_status').length === 1
        ? Promise.reject(new Error('temporary')) : Promise.resolve(running)
      return Promise.reject(new Error('unexpected command'))
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Retrying')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Waiting for Android')
    expect(wrapper.findAll('button').some(button => button.text() === 'Stop listener')).toBe(true)
    wrapper.unmount()
  })

  it('resumes status updates after a failed stop', async () => {
    const running = { running: true, sessionActive: true, status: 'Session active', invitation: null, metrics: null, volume: 100, muted: false }
    const stopped = { ...running, running: false, sessionActive: false, status: 'Stopped' }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'start_lab') return Promise.resolve(running)
      if (command === 'stop_lab') return Promise.reject('Stop response lost.')
      if (command === 'lab_status') return Promise.resolve(stopped)
      return Promise.reject(new Error('unexpected command'))
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Stopped')
    expect(wrapper.findAll('button').some(button => button.text() === 'Start listener')).toBe(true)
    wrapper.unmount()
  })
  it('loads real probe results and refreshes them on request', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] }).mockResolvedValueOnce({ generation: 2, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 0, selectedIndex: null })
    expect(wrapper.text()).toContain('Speakers (default)')
    expect(wrapper.text()).toContain('48,000 Hz · 2 channels')
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('No output devices found.')
    wrapper.unmount()
  })

  it('reports failure and allows a retry', async () => {
    const invoke = vi.fn().mockRejectedValueOnce(new Error('unavailable')).mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('Could not inspect')
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect(wrapper.find('[role="alert"]').exists()).toBe(false)
    expect(wrapper.text()).toContain('No output devices found.')
    wrapper.unmount()
  })

  it('keeps endpoints visible when an individual format is unavailable', async () => {
    window.__TAURI__ = { core: { invoke: vi.fn().mockResolvedValue({ generation: 1, devices: [
      { label: 'Unavailable', sampleRateHz: null, channels: null, isDefault: false },
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] }) } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect(wrapper.text()).toContain('Unavailable')
    expect(wrapper.text()).toContain('Default format unavailable')
    expect(wrapper.text()).toContain('Speakers (default)')
    expect(wrapper.text()).toContain('48,000 Hz · 2 channels')
    wrapper.unmount()
  })

  it('plays a test tone on the selected snapshot entry', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 7, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] }).mockResolvedValueOnce(undefined)
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenLastCalledWith('play_test_tone', { generation: 7, index: 0 })
    expect(wrapper.text()).toContain('Test tone finished')
    expect(wrapper.text()).toContain('Speakers (default)')
    wrapper.unmount()
  })

  it('disables controls during playback and reports output failures', async () => {
    let rejectPlayback!: (error: Error) => void
    const playback = new Promise<void>((_resolve, reject) => { rejectPlayback = reject })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 2, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ] }).mockReturnValueOnce(playback).mockResolvedValueOnce(undefined)
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    expect(wrapper.get('button').attributes('disabled')).toBeDefined()
    expect(wrapper.get('[aria-label="Play test tone on Speakers"]').attributes('disabled')).toBeDefined()
    rejectPlayback(new Error('device unavailable'))
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('Could not play the test tone')
    expect(wrapper.get('button').attributes('disabled')).toBeUndefined()
    await wrapper.get('[aria-label="Play test tone on Headphones"]').trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenLastCalledWith('play_test_tone', { generation: 2, index: 1 })
    expect(wrapper.text()).toContain('Test tone finished')
    wrapper.unmount()
  })

  it('shows a specific supported playback error without hiding the list', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] }).mockRejectedValueOnce('This output format is not supported.')
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toBe('This output format is not supported.')
    expect(wrapper.find('[aria-label="Play test tone on Speakers"]').exists()).toBe(true)
    wrapper.unmount()
  })
})
