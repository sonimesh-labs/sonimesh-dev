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

  it('exposes preview fault simulation only in preview mode', async () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'streaming',
        pairingState: 'consumed',
        preview: true,
        busy: false,
      },
    })

    expect(wrapper.findAll('.simulate-actions button').map((button) => button.text())).toEqual([
      'Simulate connection loss',
      'Simulate output loss',
      'Simulate session expiry',
      'Simulate command rejection',
    ])

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate connection loss')!
      .trigger('click')
    expect(wrapper.emitted('simulateConnectionLost')).toHaveLength(1)

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate output loss')!
      .trigger('click')
    expect(wrapper.emitted('simulateOutputLost')).toHaveLength(1)

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate command rejection')!
      .trigger('click')
    expect(wrapper.emitted('simulateCommandRejection')).toHaveLength(1)
  })

  it('offers reconnect and restore actions for recovery preview states', async () => {
    const reconnecting = mount(SessionPanel, {
      props: {
        state: 'reconnecting',
        pairingState: 'consumed',
        preview: true,
        busy: false,
      },
    })
    expect(reconnecting.findAll('.simulate-actions button').map((button) => button.text())).toEqual([
      'Reconnect',
      'Simulate session expiry',
      'Simulate command rejection',
    ])
    await reconnecting
      .findAll('button')
      .find((button) => button.text() === 'Reconnect')!
      .trigger('click')
    expect(reconnecting.emitted('simulateReconnected')).toHaveLength(1)

    const unavailable = mount(SessionPanel, {
      props: {
        state: 'outputUnavailable',
        pairingState: 'consumed',
        preview: true,
        busy: false,
      },
    })
    expect(unavailable.findAll('.simulate-actions button').map((button) => button.text())).toEqual([
      'Restore output',
      'Simulate session expiry',
      'Simulate command rejection',
    ])
    await unavailable
      .findAll('button')
      .find((button) => button.text() === 'Restore output')!
      .trigger('click')
    expect(unavailable.emitted('simulateOutputRestored')).toHaveLength(1)
  })

  it('hides fault simulation from the live receiver view', () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'streaming',
        pairingState: 'consumed',
        busy: false,
      },
    })

    expect(wrapper.find('.simulate-actions').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Simulate connection loss')
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

  it('emits session expiry from any trusted preview state', async () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'ready',
        pairingState: 'consumed',
        preview: true,
        busy: false,
      },
    })

    await wrapper
      .findAll('button')
      .find((button) => button.text() === 'Simulate session expiry')!
      .trigger('click')
    expect(wrapper.emitted('simulateSessionExpired')).toHaveLength(1)
  })

  it('hides session expiry simulation without a consumed pairing record', () => {
    const wrapper = mount(SessionPanel, {
      props: {
        state: 'ready',
        pairingState: 'inactive',
        preview: true,
        busy: false,
      },
    })

    expect(
      wrapper
        .findAll('.simulate-actions button')
        .map((button) => button.text())
        .includes('Simulate session expiry'),
    ).toBe(false)
  })
})
