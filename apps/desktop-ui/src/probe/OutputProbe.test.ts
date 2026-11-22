import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import OutputProbe from './OutputProbe.vue'

afterEach(() => { delete window.__TAURI__ })

describe('desktop output probe', () => {
  it('loads real probe results and refreshes them on request', async () => {
    const invoke = vi.fn().mockResolvedValueOnce([
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ]).mockResolvedValueOnce([])
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
    const invoke = vi.fn().mockRejectedValueOnce(new Error('unavailable')).mockResolvedValueOnce([])
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
    window.__TAURI__ = { core: { invoke: vi.fn().mockResolvedValue([
      { label: 'Unavailable', sampleRateHz: null, channels: null, isDefault: false },
      { label: 'Speakers', sampleRateHz: 48_000, channels: 2, isDefault: true },
    ]) } }
    const wrapper = mount(OutputProbe)
    await flushPromises()
    expect(wrapper.text()).toContain('Unavailable')
    expect(wrapper.text()).toContain('Default format unavailable')
    expect(wrapper.text()).toContain('Speakers (default)')
    expect(wrapper.text()).toContain('48,000 Hz · 2 channels')
    wrapper.unmount()
  })
})
