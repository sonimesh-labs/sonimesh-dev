import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'

import SessionPanel from './SessionPanel.vue'

describe('SessionPanel', () => {
  it('labels preview streaming as simulated and makes no audio claim', () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'streaming',
        pairingState: 'consumed',
        preview: true,
        busy: false,
      },
    })

    expect(wrapper.find('.session-kicker').text()).toBe('Preview receiver')
    expect(wrapper.find('.state-badge').text()).toBe('Preview streaming')
    expect(wrapper.get('h2').text()).toBe('Preview streaming')
    expect(wrapper.get('.session-copy p').text()).toContain('no audio is captured, transported, or played')
    expect(wrapper.text()).toContain('Preview state')
    expect(wrapper.text()).not.toContain('Audio is moving privately across your local network.')
    expect(wrapper.get('button').text()).toContain('Stop listening preview')
  })

  it('preserves receiver copy when preview is omitted', () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'streaming',
        pairingState: 'consumed',
        busy: false,
      },
    })

    expect(wrapper.find('.session-kicker').text()).toBe('Live receiver')
    expect(wrapper.find('.state-badge').text()).toBe('streaming')
    expect(wrapper.get('h2').text()).toBe('Now streaming')
    expect(wrapper.get('.session-copy p').text()).toBe('Audio is moving privately across your local network.')
  })
})
