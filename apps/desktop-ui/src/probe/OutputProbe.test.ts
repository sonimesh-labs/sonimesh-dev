import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import OutputProbe from './OutputProbe.vue'

afterEach(() => { delete window.__TAURI__ })

describe('desktop output probe', () => {
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
