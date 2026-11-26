import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import type { DesktopBridgeInvoke } from '../bridge'
import OutputProbe from './OutputProbe.vue'

vi.mock('./BluetoothProbe.vue', () => ({ default: { template: '<div />' } }))

const originalClipboard = navigator.clipboard
const originalShare = navigator.share

afterEach(() => {
  vi.useRealTimers()
  delete window.__TAURI__
  localStorage.clear()
  sessionStorage.clear()
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
  it('restores and resets the saved control port', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({ controlPort: 47612 }))
    const invoke = vi.fn((command: string) => command === 'probe_outputs'
      ? Promise.resolve({ generation: 1, devices: [] }) : Promise.resolve({ running: false }))
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-control-port').element as HTMLInputElement).value).toBe('47612')
    await wrapper.findAll('button').find(button => button.text() === 'Reset lab settings')!.trigger('click')
    expect((wrapper.get('#lab-control-port').element as HTMLInputElement).value).toBe('47610')
    wrapper.unmount()
  })

  it('validates, saves, and starts with the selected control port', async () => {
    const running = { running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: null, muted: null, controlPort: 47611 }
    const invoke = vi.fn((command: string) => command === 'probe_outputs'
      ? Promise.resolve({ generation: 1, devices: [] }) : Promise.resolve(running))
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    const port = wrapper.get('#lab-control-port')
    const start = wrapper.findAll('button').find(button => button.text() === 'Start listener')!
    for (const invalid of ['', '0', '65536', '12x']) {
      await port.setValue(invalid)
      expect(start.attributes('disabled')).toBeDefined()
      expect(port.attributes('aria-invalid')).toBe('true')
    }
    await port.setValue('47611')
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!).controlPort).toBe(47611)
    await start.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ controlPort: 47611 }))
    expect(port.attributes('disabled')).toBeDefined()
    expect(wrapper.get('#lab-control-port-help').text()).toContain('Running listener port: 47611')
    expect(wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('validates and passes the receiver port while keeping stop available', async () => {
    const running = { running: true, sessionActive: false, status: 'Waiting', invitation: null, metrics: null, volume: null, muted: null }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      return Promise.resolve(running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    const port = wrapper.get('#lab-media-port')
    expect(port.attributes('aria-describedby')).toBe('lab-media-port-help')
    expect(wrapper.get('#lab-media-port-help').text()).toContain("set Android's Desktop UDP port to this value")
    const start = wrapper.findAll('button').find(button => button.text() === 'Start listener')!
    for (const invalid of ['', '0', '65536', '12x']) {
      await port.setValue(invalid)
      expect(start.attributes('disabled')).toBeDefined()
      expect(port.attributes('aria-invalid')).toBe('true')
      expect(wrapper.text()).toContain('Enter a receiver UDP port from 1 to 65535.')
    }
    await port.setValue('39094')
    expect(start.attributes('disabled')).toBeUndefined()
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!).mediaPort).toBe(39094)
    await start.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ mediaPort: 39094 }))
    expect(port.attributes('disabled')).toBeDefined()
    expect(wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

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
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({ mediaPort: 39093, startupBufferMs: 20, adaptive: false }))
    const settings = { startupBufferMs: 80, adaptive: true }
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const invoke = vi.fn()
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', metrics: null, events: ['started'], volume: 60, muted: false, mediaPort: 39094, settings })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting for Android', invitation: '01ab', metrics: null, events: ['started'], volume: 60, muted: false, mediaPort: 39094, settings })
      .mockResolvedValueOnce({ running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, events: ['started'], volume: null, muted: null, settings })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Session active')
    expect(wrapper.text()).toContain('running listener may use different settings')
    expect(wrapper.text()).toContain('Running listener UDP port: 39094')
    expect(wrapper.text()).toContain('Running listener launch settings: 80 ms startup buffer, adaptive playout.')
    expect((wrapper.get('#lab-buffer').element as HTMLSelectElement).value).toBe('20')
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('Listener launch settings: 80 ms startup buffer; adaptive playout'))
    expect((wrapper.get('#lab-media-port').element as HTMLInputElement).value).toBe('39093')
    expect(wrapper.get('#lab-media-port-help').text()).toContain('use the running listener UDP port on Android')
    expect(invoke).not.toHaveBeenCalledWith('probe_outputs', expect.anything())
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Waiting for Android')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('stop_lab')
    expect(wrapper.text()).not.toContain('Running listener UDP port:')
    expect(wrapper.text()).toContain('Last listener launch settings: 80 ms startup buffer, adaptive playout.')
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

  it('distinguishes decoded silence from an interval without frames and clears it on session end', async () => {
    vi.useFakeTimers()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const base = { running: true, sessionActive: true, status: 'Session active', invitation: null, metrics: null, volume: 50, muted: false }
    const invoke = vi.fn()
      .mockResolvedValueOnce({ ...base, signal: null })
      .mockResolvedValueOnce({ ...base, signal: { decoded: 50, peak: 0.1 } })
      .mockResolvedValueOnce({ ...base, signal: { decoded: 50, peak: 0 } })
      .mockResolvedValueOnce({ ...base, signal: { decoded: 0, peak: null } })
      .mockResolvedValueOnce({ ...base, signal: { decoded: 0, peak: null }, signalStale: true })
      .mockResolvedValueOnce({ ...base, signal: { decoded: 50, peak: 0.1 }, signalStale: false })
      .mockResolvedValueOnce({ ...base, sessionActive: false, signal: null })
      .mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Waiting for a signal observation.')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('50 decoded frames; peak 0.100000.')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('50 decoded frames; peak 0.000000.')
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText.mock.calls[0][0]).toContain('Last decoded interval: 50 frames; peak 0.000000 (before output gain; excludes concealment)')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('No decoded frames in this interval.')
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText.mock.calls[1][0]).toContain('Last decoded interval: 0 frames; no peak')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Stale observation: no update for at least 3 seconds.')
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText.mock.calls[2][0]).toContain('Last decoded interval (stale: no update for at least 3 seconds): 0 frames; no peak')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).not.toContain('Stale observation')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).not.toContain('Last reported decoded interval')
    expect(wrapper.text()).not.toContain('Copy session evidence')
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

  it('keeps session evidence after stopping the listener and clears it on restart', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const metrics = { packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2', decodeErrors: '1', buffered: '4', output: 'active', late: '0', resyncDropped: '0', playout: 'fixed', adaptive: null }
    const running = { running: true, sessionActive: true, status: 'Session active', invitation: '01ab', events: ['RESULT receiver started'], metrics, volume: 50, muted: false }
    const stopped = { ...running, running: false, sessionActive: false, status: 'Stopped', invitation: null, volume: null, muted: null }
    const starting = { ...stopped, running: true, status: 'Starting listener…', events: [], metrics: null }
    const invoke = vi.fn((command: string) => {
      if (command === 'lab_status') return Promise.resolve(running)
      if (command === 'stop_lab') return Promise.resolve(stopped)
      if (command === 'start_lab') return Promise.resolve(starting)
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      return Promise.reject(new Error('unexpected command'))
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText.mock.calls[0][0]).toContain('Receiver controls (current): volume 50%; mute off')
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Last session observations')
    expect(wrapper.text()).toContain('Last available update')
    expect(wrapper.text()).toContain('Last reported: Active')
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('Decoded frames: 115'))
    expect(writeText.mock.calls[1][0]).not.toContain('Receiver controls (current)')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).not.toContain('Last session observations')
    expect(wrapper.text()).not.toContain('Session evidence copied.')
    wrapper.unmount()
  })

  it('explains muted and zero-volume receiver states beside decoded activity', async () => {
    vi.useFakeTimers()
    const base = { running: true, sessionActive: true, status: 'Session active', invitation: null, events: [], metrics: null, signal: { decoded: 10, peak: 0.2 }, volume: 0, muted: true }
    const invoke = vi.fn().mockResolvedValueOnce(base)
      .mockResolvedValueOnce({ ...base, muted: false })
      .mockResolvedValueOnce({ ...base, volume: 50, muted: false })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Receiver is muted; decoded signal is measured before mute.')
    expect(wrapper.text()).not.toContain('Receiver volume is 0%')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Receiver volume is 0%; decoded signal is measured before volume.')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).not.toContain('Receiver volume is 0%')
    expect(wrapper.text()).not.toContain('Receiver is muted')
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
  it('lists desktop addresses and applies only the selected address', async () => {
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'list_lab_hosts') return Promise.resolve(['10.0.0.8', '192.168.1.2'])
      return Promise.resolve({ running: true, status: 'Starting', invitation: null })
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'List desktop addresses')!.trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
    expect(wrapper.text()).toContain('VPN and virtual adapter addresses may appear')
    await wrapper.findAll('button').find(button => button.text() === 'Use 192.168.1.2')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ host: '192.168.1.2' }))
    wrapper.unmount()
  })

  it('keeps manual address entry available if address listing fails', async () => {
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'list_lab_hosts') return Promise.reject('Could not list desktop addresses.')
      return Promise.resolve({ running: false })
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-host').setValue('10.0.0.4')
    await wrapper.findAll('button').find(button => button.text() === 'List desktop addresses')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Could not list desktop addresses.')
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('10.0.0.4')
    wrapper.unmount()
  })

  it('reports an empty desktop address list', async () => {
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'list_lab_hosts') return Promise.resolve([])
      return Promise.resolve({ running: false })
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'List desktop addresses')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('No desktop IPv4 addresses found.')
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
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
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '192.168.1.10', generation: 1, index: null, startupBufferMs: 60, adaptive: false, mediaPort: 39093, controlPort: 47610, initialVolume: 100 })
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

  it.each(['success', 'failure'])('ignores an old route %s after settings are reset and a new query starts', async (outcome) => {
    let resolveOld!: (address: string) => void
    let rejectOld!: (cause: string) => void
    let resolveNew!: (address: string) => void
    const oldQuery = new Promise<string>((resolve, reject) => { resolveOld = resolve; rejectOld = reject })
    const newQuery = new Promise<string>(resolve => { resolveNew = resolve })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockReturnValueOnce(oldQuery).mockReturnValueOnce(newQuery)
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#android-host').setValue('192.168.1.20')
    await wrapper.findAll('button').find(button => button.text() === 'Suggest desktop address')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Reset lab settings')!.trigger('click')
    await wrapper.get('#android-host').setValue('10.0.0.20')
    await wrapper.findAll('button').find(button => button.text() === 'Suggest desktop address')!.trigger('click')
    if (outcome === 'success') resolveOld('192.168.1.10')
    else rejectOld('Old route failed')
    await flushPromises()
    expect(wrapper.text()).not.toContain('192.168.1.10')
    expect(wrapper.text()).not.toContain('Old route failed')
    expect(wrapper.findAll('button').find(button => button.text() === 'Checking route…')!.attributes('disabled')).toBeDefined()
    resolveNew('10.0.0.10')
    await flushPromises()
    expect(wrapper.text()).toContain('Suggested desktop address: 10.0.0.10')
    await wrapper.findAll('button').find(button => button.text() === 'Use address')!.trigger('click')
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('10.0.0.10')
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
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings') ?? '{}').initialVolume).not.toBe(35)
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Receiver volume: 35%')
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings') ?? '{}').initialVolume).toBe(35)
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

  it('sets and remembers startup volume, and resets it with lab settings', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({ initialVolume: 35 }))
    const stopped = { running: false, sessionActive: false, status: 'Stopped', invitation: null, metrics: null, volume: null, muted: null }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      if (command === 'start_lab') return Promise.resolve({ ...stopped, running: true, volume: 35, muted: false })
      return Promise.resolve(stopped)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect(wrapper.text()).toContain('Starting receiver volume: 35%')
    await wrapper.get('#lab-initial-volume').setValue('20')
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings') ?? '{}').initialVolume).toBe(20)
    wrapper.unmount()
    const reopened = mountStoppedProbe()
    await flushPromises()
    expect((reopened.get('#lab-initial-volume').element as HTMLInputElement).value).toBe('20')
    await reopened.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ initialVolume: 20 }))
    expect(reopened.find('#lab-initial-volume').exists()).toBe(false)
    await reopened.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await reopened.findAll('button').find(button => button.text() === 'Reset lab settings')!.trigger('click')
    expect(reopened.text()).toContain('Starting receiver volume: 100%')
    await reopened.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenLastCalledWith('start_lab', expect.objectContaining({ initialVolume: 100 }))
    reopened.unmount()
  })

  it('shows the selected volume while adjusting without sending it before commit', async () => {
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
    const slider = wrapper.get('#lab-volume')
    ;(slider.element as HTMLInputElement).value = '35'
    await slider.trigger('input')
    expect(wrapper.text()).toContain('Receiver volume: 100%')
    expect(wrapper.text()).toContain('Selected: 35%')
    expect(invoke.mock.calls.some(([command]) => command === 'control_lab')).toBe(false)
    await vi.advanceTimersByTimeAsync(1000)
    expect(slider.element).toHaveProperty('value', '35')
    await slider.trigger('change')
    expect(invoke).toHaveBeenCalledWith('control_lab', { volume: 35, muted: null })
    expect(slider.element).toHaveProperty('value', '100')
    expect(wrapper.text()).not.toContain('Selected: 35%')
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
    await wrapper.get('#lab-buffer').setValue('0')
    await wrapper.get('#lab-playout').setValue(true)
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '127.0.0.1', generation: 4, index: 1, startupBufferMs: 0, adaptive: true, mediaPort: 39093, controlPort: 47610, initialVolume: 100 })
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
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '127.0.0.1', generation: 5, index: 0, startupBufferMs: 60, adaptive: false, mediaPort: 39093, controlPort: 47610, initialVolume: 100 })
    wrapper.unmount()
  })

  it('restores the same endpoint after it disappears and the view reopens', async () => {
    const chosen = { id: 'chosen-id', label: 'USB DAC', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const other = { id: 'other-id', label: 'USB DAC', sampleRateHz: 48_000, channels: 2, isDefault: true }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [chosen, other] })
      .mockResolvedValueOnce({ generation: 2, devices: [other], selectedIndex: null })
      .mockResolvedValueOnce({ generation: 3, devices: [other, chosen], selectedIndex: null })
      .mockResolvedValueOnce({ running: true, status: 'Starting', invitation: null, metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const first = mountStoppedProbe()
    await flushPromises()
    await first.get('#lab-output').setValue('0')
    expect(sessionStorage.getItem('sonimesh.outputProbe.outputId')).toBe('chosen-id')
    expect(localStorage.getItem('sonimesh.outputProbe.labSettings')).not.toContain('chosen-id')
    await first.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect((first.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(first.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    first.unmount()

    const reopened = mountStoppedProbe()
    await flushPromises()
    expect((reopened.get('#lab-output').element as HTMLSelectElement).value).toBe('1')
    await reopened.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ generation: 3, index: 1 }))
    reopened.unmount()
  })

  it('keeps same-name output labels and test controls distinct across reordering', async () => {
    const first = { id: 'a-id', label: 'USB DAC', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const second = { id: 'b-id', label: 'USB DAC', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [second, first] })
      .mockResolvedValueOnce({ generation: 2, devices: [first, second] })
      .mockResolvedValueOnce(undefined)
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    let options = wrapper.get('#lab-output').findAll('option')
    expect(options[1].text()).toBe('USB DAC (2)')
    expect(options[2].text()).toBe('USB DAC (1)')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    options = wrapper.get('#lab-output').findAll('option')
    expect(options[1].text()).toBe('USB DAC (1)')
    expect(options[2].text()).toBe('USB DAC (2)')
    await wrapper.get('[aria-label="Play test tone on USB DAC (2)"]').trigger('click')
    expect(invoke).toHaveBeenCalledWith('play_test_tone', { generation: 2, index: 1 })
    wrapper.unmount()
  })

  it('restores the chosen endpoint after a failed refresh and clears it on default selection', async () => {
    const chosen = { id: 'chosen-id', label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [chosen] })
      .mockRejectedValueOnce(new Error('Unavailable'))
      .mockResolvedValueOnce({ generation: 2, devices: [chosen] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('0')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('0')
    await wrapper.get('#lab-output').setValue('')
    expect(sessionStorage.getItem('sonimesh.outputProbe.outputId')).toBeNull()
    wrapper.unmount()
  })

  it('does not restore an endpoint when its ID appears more than once', async () => {
    const chosen = { id: 'chosen-id', label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [chosen] })
      .mockResolvedValueOnce({ generation: 2, devices: [chosen, chosen], selectedIndex: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('0')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('restores the selected endpoint by device details after the list is reordered', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({
      labOutput: '0',
      labOutputDevice: { label: 'Headphones', sampleRateHz: 48_000, channels: 2 },
    }))
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 0, selectedIndex: null })
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('1')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })

  it('keeps the system default after selecting it and reopening the probe', async () => {
    const devices = [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ]
    const invoke = vi.fn((command: string) => Promise.resolve(command === 'start_lab'
      ? { running: true, sessionActive: false, status: 'Starting', invitation: null, metrics: null, volume: null, muted: null }
      : { generation: 1, devices, selectedIndex: null }))
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.get('#lab-output').setValue('1')
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!).labOutputDevice.label).toBe('Headphones')
    await wrapper.get('#lab-output').setValue('')
    await wrapper.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenLastCalledWith('probe_outputs', { generation: 1, selectedIndex: null })
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!).labOutputDevice).toBeUndefined()
    wrapper.unmount()

    const reopened = mountStoppedProbe()
    await flushPromises()
    expect((reopened.get('#lab-output').element as HTMLSelectElement).value).toBe('')
    await reopened.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', expect.objectContaining({ index: null }))
    reopened.unmount()
  })

  it('honors a previously saved default even if an old device description remains', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({
      labOutput: '', labOutputDevice: { label: 'Headphones', sampleRateHz: 48_000, channels: 2 },
    }))
    const invoke = vi.fn().mockResolvedValue({ generation: 1, devices: [
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('')
    wrapper.unmount()
  })

  it('requires reselection when a saved output description is ambiguous', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({
      labOutput: '0',
      labOutputDevice: { label: 'USB DAC', sampleRateHz: 48_000, channels: 2 },
    }))
    const device = { label: 'USB DAC', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [device, device] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(wrapper.text()).toContain('Select an available output before starting the listener.')
    wrapper.unmount()
  })

  it('requires reselection for legacy saved output indexes without device details', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({ labOutput: '0' }))
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(wrapper.findAll('button').find(button => button.text() === 'Start listener')!.attributes('disabled')).toBeDefined()
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
  it.each(['format unavailable', 'refresh failed'])('retains the reselection requirement after reopening when %s', async (failure) => {
    const device = { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [device], selectedIndex: null })
    if (failure === 'refresh failed') invoke.mockRejectedValueOnce(new Error('unavailable'))
    else invoke.mockResolvedValueOnce({ generation: 2, devices: [{ ...device, sampleRateHz: null }], selectedIndex: 0 })
    invoke.mockResolvedValueOnce({ generation: 3, devices: [device], selectedIndex: null })
    window.__TAURI__ = { core: { invoke } }
    const first = mountStoppedProbe()
    await flushPromises()
    await first.get('#lab-output').setValue('0')
    await first.findAll('button').find(button => button.text() === 'Refresh devices')!.trigger('click')
    await flushPromises()
    first.unmount()

    const reopened = mountStoppedProbe()
    await flushPromises()
    expect((reopened.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    const start = reopened.findAll('button').find(button => button.text() === 'Start listener')!
    expect(start.attributes('disabled')).toBeDefined()
    await reopened.get('#lab-output').setValue('')
    expect(start.attributes('disabled')).toBeUndefined()
    expect(JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!).labOutput).toBe('')
    reopened.unmount()
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
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '10.0.2.2', generation: 1, index: null, startupBufferMs: 60, adaptive: false, mediaPort: 39093, controlPort: 47610, initialVolume: 100 })
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

  it('uses the running direct port consistently for display, copy and share', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    const share = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    Object.defineProperty(navigator, 'share', { configurable: true, value: share })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, status: 'Waiting for Android', invitation: '01ab', mediaPort: 39094 })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect((wrapper.get('#lab-direct-port').element as HTMLInputElement).checked).toBe(true)
    expect((wrapper.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('SONIMESH-LAB1:39094:01ab')
    await wrapper.findAll('button').find(button => button.text() === 'Copy invitation')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Share invitation')!.trigger('click')
    expect(writeText).toHaveBeenCalledWith('SONIMESH-LAB1:39094:01ab')
    expect(share).toHaveBeenCalledWith({ title: 'SoniMesh lab invitation', text: 'SONIMESH-LAB1:39094:01ab' })
    expect(wrapper.get('.lab-invitation-qr').attributes('src')).toMatch(/^data:image\/svg\+xml,/)
    await wrapper.get('#lab-direct-port').setValue(false)
    expect((wrapper.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('01ab')
    await wrapper.findAll('button').find(button => button.text() === 'Copy invitation')!.trigger('click')
    expect(writeText).toHaveBeenLastCalledWith('01ab')
    wrapper.unmount()
  })

  it('retains relay mode on reopening and resets it after the listener stops', async () => {
    const running = { running: true, sessionActive: false, status: 'Waiting', invitation: '01ab', metrics: null, volume: null, muted: null, mediaPort: 39094 }
    const stopped = { ...running, running: false, invitation: null }
    const invoke = vi.fn((command: string) => {
      if (command === 'probe_outputs') return Promise.resolve({ generation: 1, devices: [] })
      return Promise.resolve(command === 'stop_lab' ? stopped : running)
    })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const first = mount(OutputProbe)
    await flushPromises()
    await first.get('#lab-direct-port').setValue(false)
    first.unmount()
    const reopened = mount(OutputProbe)
    await flushPromises()
    expect((reopened.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('01ab')
    await reopened.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    await reopened.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect((reopened.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('SONIMESH-LAB1:39094:01ab')
    reopened.unmount()
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

  it('falls back to copying the invitation when system sharing is unavailable', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'share', { configurable: true, value: undefined })
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
    await wrapper.findAll('button').find(button => button.text() === 'Share invitation')!.trigger('click')
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith('01ab')
    expect(wrapper.text()).toContain('invitation copied instead')
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
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: 'secret-invitation', events: ['READY', 'RESULT receiver started', 'OUTPUT active'], metrics })
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
    expect(copied).toContain('Recent listener events:')
    expect(copied).toContain('- READY')
    expect(copied).toContain('- RESULT receiver started')
    expect(copied).not.toContain('secret-invitation')
    expect(wrapper.text()).toContain('Session evidence copied.')
    wrapper.unmount()
  })

  it('shares bounded session evidence through the system share API', async () => {
    const share = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'share', { configurable: true, value: share })
    const metrics = { packets: '8', rejected: '1', decoded: '7', peak: '0.125', plc: '0', decodeErrors: '0', buffered: '2', output: 'active', media: 'receiving', adaptive: null }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: 'secret-invitation', events: ['READY'], metrics })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Share session evidence')!.trigger('click')
    await flushPromises()
    expect(share).toHaveBeenCalledWith(expect.objectContaining({ title: 'SoniMesh session evidence' }))
    expect(share.mock.calls[0][0].text).toContain('Accepted packets: 8')
    expect(share.mock.calls[0][0].text).toContain('- READY')
    expect(share.mock.calls[0][0].text).not.toContain('secret-invitation')
    expect(wrapper.text()).toContain('Session evidence shared.')
    wrapper.unmount()
  })

  it('bounds listener events in copied evidence and omits the section when none exist', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const longEvent = `RESULT ${'x'.repeat(120)}`
    const metrics = { packets: '1', rejected: '0', decoded: '1', peak: '0.100', plc: '0', decodeErrors: '0', buffered: '0', output: 'active', media: 'receiving', adaptive: null }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: null, events: [longEvent], metrics })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    await flushPromises()
    const copied = writeText.mock.calls[0][0] as string
    expect(copied).toContain('Recent listener events:')
    expect(copied).toContain(`- ${longEvent.slice(0, 77)}...`)
    expect(copied).not.toContain(longEvent)
    wrapper.unmount()

    const noEventsInvoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: null, metrics })
    window.__TAURI__ = { core: { invoke: noEventsInvoke as DesktopBridgeInvoke } }
    const noEventsWrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await noEventsWrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await noEventsWrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    await flushPromises()
    expect(writeText.mock.calls[1][0]).not.toContain('Recent listener events:')
    noEventsWrapper.unmount()
  })

  it('copies event-only evidence when a session ends before metrics arrive', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Session failed; waiting for Android', invitation: null, events: ['RESULT client connected', 'RESULT session failed'], metrics: null })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Session evidence')
    expect(wrapper.find('.lab-metrics').exists()).toBe(false)
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining('Status: Session failed; waiting for Android'))
    expect(writeText.mock.calls[0][0]).toContain('- RESULT session failed')
    wrapper.unmount()
  })

  it('falls back to copying session evidence when system sharing is unavailable', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'share', { configurable: true, value: undefined })
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const metrics = { packets: '4', rejected: '0', decoded: '4', peak: '0.200', plc: '0', decodeErrors: '0', buffered: '1', output: 'active', media: 'receiving', adaptive: null }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: null, metrics })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Share session evidence')!.trigger('click')
    await flushPromises()
    expect(writeText.mock.calls[0][0]).toContain('Decoded frames: 4')
    expect(wrapper.text()).toContain('session evidence copied instead')
    wrapper.unmount()
  })

  it('clears a copied evidence notice when a new session starts', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    const metrics = { packets: '1', rejected: '0', decoded: '1', peak: '0.1', plc: '0', decodeErrors: '0', buffered: '0', output: 'active', media: 'receiving', adaptive: null }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Starting', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', metrics })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Waiting for Android', invitation: '01ab', metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'New session active', invitation: '01ab', metrics })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    await wrapper.findAll('button').find(button => button.text() === 'Copy session evidence')!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Session evidence copied.')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).not.toContain('Session evidence copied.')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('New session active')
    expect(wrapper.text()).not.toContain('Session evidence copied.')
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

  it('restores lab settings and requires reselection when the saved device snapshot is stale', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({
      labHost: '192.168.1.7', androidHost: '192.168.1.22', labOutput: '1', startupBufferMs: 0, adaptive: true, mediaPort: 39094,
    }))
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 3, devices: [
      { label: 'Default', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'USB', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ], selectedIndex: null })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('192.168.1.7')
    expect((wrapper.get('#android-host').element as HTMLInputElement).value).toBe('192.168.1.22')
    expect((wrapper.get('#lab-buffer').element as HTMLSelectElement).value).toBe('0')
    expect((wrapper.get('#lab-media-port').element as HTMLInputElement).value).toBe('39094')
    expect((wrapper.get('#lab-playout').element as HTMLSelectElement).value).toBe('true')
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('unavailable')
    expect(invoke).toHaveBeenCalledWith('probe_outputs', { generation: 0, selectedIndex: null })
    wrapper.unmount()
  })

  it('ignores malformed persisted lab settings and stores bounded edits', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', '{"labHost":{},"labOutput":"999x","startupBufferMs":999,"adaptive":"yes"')
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
    expect((wrapper.get('#lab-buffer').element as HTMLSelectElement).value).toBe('60')
    await wrapper.get('#lab-host').setValue('10.0.0.8')
    await wrapper.get('#lab-buffer').setValue('0')
    await wrapper.get('#lab-playout').setValue('true')
    const stored = JSON.parse(localStorage.getItem('sonimesh.outputProbe.labSettings')!)
    expect(stored.labHost).toBe('10.0.0.8')
    expect(stored.startupBufferMs).toBe(0)
    expect(stored.adaptive).toBe(true)
    wrapper.unmount()
  })

  it('resets persisted lab settings without touching listener state', async () => {
    localStorage.setItem('sonimesh.outputProbe.labSettings', JSON.stringify({
      labHost: '192.168.1.7', androidHost: '192.168.1.22', labOutput: '1', startupBufferMs: 100, adaptive: true, mediaPort: 39094,
    }))
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
    window.__TAURI__ = { core: { invoke: invoke as DesktopBridgeInvoke } }
    const wrapper = mountStoppedProbe()
    await flushPromises()
    await wrapper.findAll('button').find(button => button.text() === 'Reset lab settings')!.trigger('click')
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
    expect((wrapper.get('#android-host').element as HTMLInputElement).value).toBe('')
    expect((wrapper.get('#lab-buffer').element as HTMLSelectElement).value).toBe('60')
    expect((wrapper.get('#lab-media-port').element as HTMLInputElement).value).toBe('39093')
    expect((wrapper.get('#lab-playout').element as HTMLSelectElement).value).toBe('false')
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('')
    expect(localStorage.getItem('sonimesh.outputProbe.labSettings')).toContain('127.0.0.1')
    expect(invoke).not.toHaveBeenCalledWith('stop_lab')
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
