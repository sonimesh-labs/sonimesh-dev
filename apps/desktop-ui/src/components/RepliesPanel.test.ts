import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'

import RepliesPanel from './RepliesPanel.vue'
import type { ControlReply } from '../types'

function reply(overrides: Partial<ControlReply> = {}): ControlReply {
  return {
    kind: 'streamStatus',
    requestId: 1,
    detail: 'stream started',
    state: 'streaming',
    ...overrides,
  }
}

describe('RepliesPanel', () => {
  it('shows an empty state when no replies are retained', () => {
    const wrapper = mount(RepliesPanel, { props: { replies: [], preview: true } })

    expect(wrapper.get('h2').text()).toBe('Recent control replies')
    expect(wrapper.text()).toContain('No control replies yet.')
  })

  it('renders the most recent stream-status reply first with its state', () => {
    const replies: ControlReply[] = [
      reply({ requestId: 2, detail: 'stream stopped', state: 'ready' }),
      reply({ requestId: 1, detail: 'stream started', state: 'streaming' }),
    ]
    const wrapper = mount(RepliesPanel, { props: { replies, preview: true } })

    const rows = wrapper.findAll('.reply-row')
    expect(rows).toHaveLength(2)
    expect(rows[0].text()).toContain('stream stopped')
    expect(rows[0].text()).toContain('state ready')
    expect(rows[0].text()).toContain('#2')
    expect(rows[1].text()).toContain('state streaming')
    expect(rows[1].text()).toContain('#1')
  })

  it('renders a structured error reply with its code and retry contract', () => {
    const replies: ControlReply[] = [
      reply({
        kind: 'error',
        requestId: 3,
        detail: 'preview command rejection',
        code: 'retryLater',
        retryable: true,
        state: undefined,
      }),
    ]
    const wrapper = mount(RepliesPanel, { props: { replies, preview: true } })

    const row = wrapper.get('.reply-row')
    expect(row.classes()).toContain('reply-error')
    expect(row.text()).toContain('preview command rejection')
    expect(row.text()).toContain('retryLater · retryable')
    expect(row.text()).toContain('#3')
  })

  it('uses preview copy only when the preview flag is set', () => {
    const wrapper = mount(RepliesPanel, { props: { replies: [], preview: false } })

    expect(wrapper.get('.eyebrow').text()).toBe('Control answers')
  })
})