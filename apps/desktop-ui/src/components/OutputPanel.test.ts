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
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.find('.eyebrow').text()).toBe('Output preview')
    expect(wrapper.get('h2').text()).toBe('Simulated output')
    expect(wrapper.text()).toContain('No hardware is inspected or changed.')
    expect(wrapper.get('label').text()).toBe('Change simulated output')
  })
})
