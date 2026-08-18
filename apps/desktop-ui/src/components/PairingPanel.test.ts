import { nextTick } from 'vue'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { enableAutoUnmount, mount } from '@vue/test-utils'

import PairingPanel from './PairingPanel.vue'
import type { PairingStatus } from '../types'

const activePairing = (expiresInSeconds: number): PairingStatus => ({
  state: 'active',
  expiresInSeconds,
})

const expiredPairing: PairingStatus = {
  state: 'expired',
  expiresInSeconds: 0,
}

enableAutoUnmount(afterEach)

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
})

describe('PairingPanel', () => {
  it('refreshes authoritative status once at the displayed deadline without expiring the invitation', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-08-05T00:00:00Z'))
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: activePairing(1),
        busy: false,
      },
    })

    await vi.advanceTimersByTimeAsync(999)
    expect(wrapper.emitted('refresh')).toBeUndefined()

    await vi.advanceTimersByTimeAsync(1)
    expect(wrapper.emitted('refresh')).toHaveLength(1)
    expect(wrapper.emitted('expire')).toBeUndefined()

    await vi.advanceTimersByTimeAsync(60_000)
    expect(wrapper.emitted('refresh')).toHaveLength(1)
  })

  it('refreshes immediately for an already-expired active snapshot', () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-08-05T00:00:00Z'))
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: activePairing(0),
        busy: false,
      },
    })

    expect(wrapper.emitted('refresh')).toHaveLength(1)
    expect(wrapper.emitted('expire')).toBeUndefined()
  })

  it('rearms the one-shot status refresh after an invitation leaves active state', async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-08-05T00:00:00Z'))
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: activePairing(0),
        busy: false,
      },
    })

    await wrapper.setProps({ pairing: expiredPairing })
    await wrapper.setProps({ pairing: activePairing(0) })

    expect(wrapper.emitted('refresh')).toHaveLength(2)
  })

  it('labels synthetic pairing state as preview-only instead of claiming sender verification', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'consumed', expiresInSeconds: null },
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.find('.eyebrow').text()).toBe('Pairing preview')
    expect(wrapper.get('h2').text()).toBe('Simulate sender trust')
    expect(wrapper.text()).toContain('Preview trust state')
    expect(wrapper.text()).toContain('verifies no real invitation')
    expect(wrapper.text()).not.toContain('Sender verified')
  })

  it('offers revoke for a consumed trust record while creation stays disabled', async () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'consumed', expiresInSeconds: null },
        busy: false,
      },
    })

    expect(wrapper.get('button.secondary-action').attributes('disabled')).toBeDefined()
    await wrapper.get('button[aria-label="Revoke trust"]').trigger('click')
    expect(wrapper.emitted('revoke')).toHaveLength(1)
  })

  it('labels the preview revoke action as preview-only', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'consumed', expiresInSeconds: null },
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.find('button[aria-label="Revoke preview trust"]').exists()).toBe(true)
    expect(wrapper.text()).toContain('Preview trust state')
  })

  it('enables creation from a revoked trust record without a revoke button', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'revoked', expiresInSeconds: null },
        busy: false,
      },
    })

    expect(wrapper.find('button[aria-label="Revoke trust"]').exists()).toBe(false)
    expect(wrapper.get('button.secondary-action').attributes('disabled')).toBeUndefined()
  })

  it('keeps revoke hidden while an invitation is active', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'active', expiresInSeconds: 60 },
        busy: false,
      },
    })

    expect(wrapper.find('button[aria-label="Revoke trust"]').exists()).toBe(false)
    expect(wrapper.find('button[aria-label="Expire invitation"]').exists()).toBe(true)
  })

  it('uses elapsed time rather than interval count when a callback is delayed', async () => {
    let nowMillis = Date.parse('2026-08-05T00:00:00Z')
    let intervalCallback: (() => void) | undefined
    vi.spyOn(Date, 'now').mockImplementation(() => nowMillis)
    vi.spyOn(window, 'setInterval').mockImplementation((callback) => {
      intervalCallback = callback as () => void
      return 1
    })

    const wrapper = mount(PairingPanel, {
      props: {
        pairing: activePairing(5),
        busy: false,
      },
    })

    nowMillis += 3_500
    intervalCallback?.()
    await nextTick()

    expect(wrapper.text()).toContain('2s remaining')
  })

  it('refreshes authoritative state when the countdown clock regresses', async () => {
    let nowMillis = Date.parse('2026-08-05T00:00:00Z')
    let intervalCallback: (() => void) | undefined
    vi.spyOn(Date, 'now').mockImplementation(() => nowMillis)
    vi.spyOn(window, 'setInterval').mockImplementation((callback) => {
      intervalCallback = callback as () => void
      return 1
    })

    const wrapper = mount(PairingPanel, {
      props: {
        pairing: activePairing(5),
        busy: false,
      },
    })

    nowMillis += 1_000
    intervalCallback?.()
    nowMillis -= 1_500
    intervalCallback?.()
    await nextTick()

    expect(wrapper.text()).toContain('0s remaining')
    expect(wrapper.emitted('refresh')).toHaveLength(1)
    expect(wrapper.emitted('expire')).toBeUndefined()
  })

  it('offers a preview-only sender-verification simulation while an invitation is active', async () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'active', expiresInSeconds: 60 },
        busy: false,
        preview: true,
      },
    })

    const button = wrapper.find('button.preview-action')
    expect(button.exists()).toBe(true)
    expect(button.text()).toBe('Simulate sender verification')
    await button.trigger('click')
    expect(wrapper.emitted('simulatePairingConsumed')).toHaveLength(1)
  })

  it('hides the sender-verification simulation outside the preview adapter', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'active', expiresInSeconds: 60 },
        busy: false,
      },
    })

    expect(wrapper.find('button.preview-action').exists()).toBe(false)
  })

  it('hides the sender-verification simulation once the invitation is no longer active', () => {
    const wrapper = mount(PairingPanel, {
      props: {
        pairing: { state: 'expired', expiresInSeconds: 0 },
        busy: false,
        preview: true,
      },
    })

    expect(wrapper.find('button.preview-action').exists()).toBe(false)
  })
})
