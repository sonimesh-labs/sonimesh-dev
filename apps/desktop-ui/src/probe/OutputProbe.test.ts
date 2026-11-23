import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import OutputProbe from './OutputProbe.vue'

afterEach(() => { vi.useRealTimers(); delete window.__TAURI__ })

describe('desktop output probe', () => {
  it('suggests a route address only when requested and applies it explicitly', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce('192.168.1.10')
      .mockResolvedValueOnce({ running: true, status: 'Starting', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await wrapper.get('#android-host').setValue(' 192.168.1.20 ')
    await wrapper.findAll('button').find(button => button.text() === 'Suggest desktop address')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('suggest_lab_host', { androidHost: '192.168.1.20' })
    expect((wrapper.get('#lab-host').element as HTMLInputElement).value).toBe('127.0.0.1')
    await wrapper.findAll('button').find(button => button.text() === 'Use address')!.trigger('click')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '192.168.1.10', generation: 1, index: null, startupBufferMs: 60 })
    wrapper.unmount()
  })

  it('shows route errors and preserves the manually entered address', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockRejectedValueOnce('No network route to that address. Enter the desktop address manually.')
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
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
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce({ running: true, status: 'Waiting', invitation: null, metrics: null, volume: 35, muted: true })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await wrapper.get('#lab-volume').setValue('35')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('control_lab', { volume: 35, muted: null })
    expect(wrapper.text()).toContain('Receiver volume: 100%')
    expect((wrapper.get('#lab-volume').element as HTMLInputElement).value).toBe('100')
    await wrapper.findAll('button').find(button => button.text() === 'Mute')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('control_lab', { volume: null, muted: true })
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Receiver volume: 35%')
    expect((wrapper.get('#lab-volume').element as HTMLInputElement).value).toBe('35')
    expect(wrapper.findAll('button').some(button => button.text() === 'Unmute')).toBe(true)
    wrapper.unmount()
  })
  it('uses the selected device snapshot for the listener and resets it on refresh', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 4, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
      { label: 'Headphones', sampleRateHz: 48_000, channels: 2, isDefault: false },
    ] }).mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: false, status: 'Stopped', invitation: null, metrics: null })
      .mockResolvedValueOnce({ generation: 5, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await wrapper.get('#lab-output').setValue('1')
    await wrapper.get('#lab-buffer').setValue('40')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '127.0.0.1', generation: 4, index: 1, startupBufferMs: 40 })
    expect(wrapper.get('#lab-buffer').attributes('disabled')).toBeDefined()
    expect(wrapper.get('#lab-output').attributes('disabled')).toBeDefined()
    expect(wrapper.get('button').attributes('disabled')).toBeDefined()
    await wrapper.findAll('button').find(button => button.text() === 'Stop listener')!.trigger('click')
    await flushPromises()
    await wrapper.get('button').trigger('click')
    await flushPromises()
    expect((wrapper.get('#lab-output').element as HTMLSelectElement).value).toBe('')
    wrapper.unmount()
  })
  it('starts and stops the lab listener with the specified control address', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
      .mockResolvedValueOnce({ running: false, status: 'Stopped', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await wrapper.get('#lab-host').setValue('10.0.2.2')
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('start_lab', { host: '10.0.2.2', generation: 1, index: null, startupBufferMs: 60 })
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
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect((wrapper.get('#lab-invitation').element as HTMLTextAreaElement).value).toBe('01ab')
    expect(wrapper.get('.lab-invitation-qr').attributes('src')).toMatch(/^data:image\/svg\+xml,/)
    expect(wrapper.text()).toContain('Waiting for Android')
    wrapper.unmount()
  })

  it('shows live and final session observations, then clears them on the next client', async () => {
    const metrics = { packets: '120', rejected: '3', decoded: '115', peak: '0.250', plc: '2', decodeErrors: '1', buffered: '4', output: 'recovering' }
    const finalMetrics = { ...metrics, decoded: '120', output: 'stopped' }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, status: 'Starting listener…', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: true, status: 'Session active', invitation: '01ab', metrics })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Session ended; waiting for Android', invitation: '01ab', metrics: finalMetrics })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Android connecting', invitation: '01ab', metrics: null })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    vi.useFakeTimers()
    await wrapper.findAll('button').find(button => button.text() === 'Start listener')!.trigger('click')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.get('.lab-metrics').text()).toContain('Accepted packets120')
    expect(wrapper.get('.lab-metrics').text()).toContain('Decoded peak0.250')
    expect(wrapper.get('.lab-metrics').text()).toContain('OutputRecovering')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.text()).toContain('Last session observations')
    expect(wrapper.text()).toContain('Final update after the stop drain')
    expect(wrapper.get('.lab-metrics').text()).toContain('Decoded frames120')
    expect(wrapper.get('.lab-metrics').text()).toContain('OutputStopped')
    await vi.advanceTimersByTimeAsync(1000)
    expect(wrapper.find('.lab-metrics').exists()).toBe(false)
    expect(wrapper.text()).toContain('Android connecting')
    wrapper.unmount()
  })

  it('labels a failed session’s last update as potentially incomplete', async () => {
    const metrics = { packets: '5', rejected: '0', decoded: '4', peak: '0.100', plc: '1', decodeErrors: '0', buffered: '0', output: 'active' }
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [] })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Starting', invitation: null, metrics: null })
      .mockResolvedValueOnce({ running: true, sessionActive: false, status: 'Session failed; waiting for Android', invitation: null, metrics })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
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
  it('loads real probe results and refreshes them on request', async () => {
    const invoke = vi.fn().mockResolvedValueOnce({ generation: 1, devices: [
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ] }).mockResolvedValueOnce({ generation: 2, devices: [] })
    window.__TAURI__ = { core: { invoke } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(invoke).toHaveBeenCalledWith('probe_outputs')
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
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
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
    const wrapper = mount(OutputProbe)
    await flushPromises()
    await wrapper.get('[aria-label="Play test tone on Speakers"]').trigger('click')
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toBe('This output format is not supported.')
    expect(wrapper.find('[aria-label="Play test tone on Speakers"]').exists()).toBe(true)
    wrapper.unmount()
  })
})
