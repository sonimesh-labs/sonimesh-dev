import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'

import OutputPanel from './OutputPanel.vue'

describe('OutputPanel', () => {
  it('labels synthetic devices as preview output', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [
          { id: 'default', label: 'Default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
        ],
        selectedId: 'default',
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.find('.eyebrow').text()).toBe('Output preview')
    expect(wrapper.get('h2').text()).toBe('Simulated output')
    expect(wrapper.text()).toContain('No hardware is inspected or changed.')
    expect(wrapper.get('label[for="output-device"]').text()).toBe('Change simulated output')
  })

  it('renders the linear gain as a percentage label', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        gain: 0.35,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.get('.volume-value').text()).toBe('35%')
  })

  it('emits a bounded linear gain when the volume slider moves', async () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    await wrapper.get('input[type="range"]').setValue('0.35')

    expect(wrapper.emitted('gain')).toEqual([[0.35]])
  })

  it('emits the opposite mute flag when the mute toggle is pressed', async () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    await wrapper.get('.mute-toggle').trigger('click')

    expect(wrapper.emitted('mute')).toEqual([[true]])
  })

  it('labels a muted volume as muted and offers unmute', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        gain: 0.4,
        muted: true,
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.get('.volume-value').text()).toBe('Muted')
    expect(wrapper.get('.mute-toggle').text()).toContain('Unmute')
  })

})
