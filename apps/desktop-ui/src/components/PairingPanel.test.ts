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
})
