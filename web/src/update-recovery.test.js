// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { buildIdentity, canRefreshUpdate, recovery, refreshUpdatedPage, startUpdateRecovery } from './system/updateRecovery'
import { session } from './auth'

const info = (version, boot = 'boot') => ({ app: { version, commit: version, built_at: 'now' }, startup: { boot_id: boot } })
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); session.username = null })

describe('global update recovery', () => {
  it('refreshes only a committed new build, never a candidate or rollback', () => {
    const before = buildIdentity(info('1.0.0'))
    expect(canRefreshUpdate(before, info('1.1.0'), { state: 'idle' })).toBe(true)
    for (const state of ['installing', 'restarting', 'rolling_back', 'failed']) {
      expect(canRefreshUpdate(before, info('1.1.0'), { state })).toBe(false)
    }
    expect(canRefreshUpdate(before, info('1.0.0', 'new-boot'), { state: 'idle' })).toBe(false)
    expect(canRefreshUpdate(before, info('1.1.0'), { state: 'idle', last_error: { code: 'failed' } })).toBe(false)
  })
  it('honors editor veto before reloading', () => {
    const guard = event => event.preventDefault()
    window.addEventListener('gamer-before-update-reload', guard)
    expect(refreshUpdatedPage()).toBe(false)
    expect(recovery.message).toContain('未保存')
    window.removeEventListener('gamer-before-update-reload', guard)
  })
  it('keeps retrying offline and restores connections after the service returns', async () => {
    vi.useFakeTimers()
    session.username = 'admin'
    let online = true
    const fetch = vi.fn(async url => {
      if (!online) throw new Error('offline')
      return { ok: true, json: async () => url.endsWith('/info') ? info('1.0.0') : { state: 'idle' } }
    })
    vi.stubGlobal('fetch', fetch)
    const restored = vi.fn()
    window.addEventListener('gamer-service-restored', restored)
    const stop = startUpdateRecovery()
    await vi.advanceTimersByTimeAsync(0)
    online = false
    await vi.advanceTimersByTimeAsync(10000)
    expect(recovery.waiting).toBe(true)
    online = true
    await vi.advanceTimersByTimeAsync(2000)
    expect(restored).toHaveBeenCalledOnce()
    expect(recovery.waiting).toBe(false)
    stop()
    const calls = fetch.mock.calls.length
    await vi.advanceTimersByTimeAsync(20000)
    expect(fetch.mock.calls.length).toBe(calls)
    window.removeEventListener('gamer-service-restored', restored)
  })
})
