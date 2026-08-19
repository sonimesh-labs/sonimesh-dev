import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'

import OutputPanel from './OutputPanel.vue'
import type { OutputSnapshot } from '../types'

function outputSnapshot(overrides: Partial<OutputSnapshot> = {}): OutputSnapshot {
  return {
    selectionRetained: true,
    streamActive: false,
    faultReported: false,
    health: 'healthy',
    ...overrides,
  }
}

describe('OutputPanel', () => {
  it('labels synthetic devices as preview output', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [
          { id: 'default', label: 'Default output', sampleRateHz: 48_000, channels: 2, isDefault: true },
        ],
        selectedId: 'default',
        output: outputSnapshot(),
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
        output: outputSnapshot(),
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
        output: outputSnapshot(),
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
        output: outputSnapshot(),
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
        output: outputSnapshot(),
        gain: 0.4,
        muted: true,
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.get('.volume-value').text()).toBe('Muted')
    expect(wrapper.get('.mute-toggle').text()).toContain('Unmute')
  })

  it('shows a neutral idle output with no fault', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot(),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    const status = wrapper.get('.output-status')
    expect(status.attributes('aria-label')).toBe('Output status')
    expect(status.classes()).toContain('health-idle')
    expect(status.text()).toContain('Stream idle · No active output')
    expect(status.text()).toContain('No output stream is currently running.')
    expect(status.text()).not.toContain('stream fault was reported')
  })

  it('does not present stale health after an explicit stop as a fault', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot({ health: 'streamError' }),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    const status = wrapper.get('.output-status')
    expect(status.classes()).toContain('health-idle')
    expect(status.text()).toContain('Stream idle · No active output')
    expect(status.text()).not.toContain('Stream error')
    expect(status.text()).not.toContain('asynchronous stream fault')
  })

  it('fails closed when a reported fault is paired with healthy health', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot({ faultReported: true }),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    const status = wrapper.get('.output-status')
    expect(status.classes()).toContain('health-streamError')
    expect(status.text()).toContain('Stream idle · Stream error')
    expect(status.text()).toContain('A stream fault was reported; reselect or reconnect the output.')
  })

  it('shows an active stream when the controller owns the output', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot({ streamActive: true }),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.get('.output-status').text()).toContain('Stream active · Healthy')
  })

  it('reports a stream error and the retained fault', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot({ streamActive: false, faultReported: true, health: 'streamError' }),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    const status = wrapper.get('.output-status')
    expect(status.classes()).toContain('health-streamError')
    expect(status.text()).toContain('Stream idle · Stream error')
    expect(status.text()).toContain('A stream fault was reported; reselect or reconnect the output.')
  })

  it('labels a callback capacity fault without a retained fault note', () => {
    const wrapper = mount(OutputPanel, {
      props: {
        devices: [],
        selectedId: null,
        output: outputSnapshot({ streamActive: true, health: 'callbackCapacityExceeded' }),
        gain: 1,
        muted: false,
        busy: false,
        preview: true,
      },
    })

    const status = wrapper.get('.output-status')
    expect(status.classes()).toContain('health-callbackCapacityExceeded')
    expect(status.text()).toContain('Callback capacity exceeded')
    expect(status.text()).not.toContain('stream fault was reported')
  })

})
