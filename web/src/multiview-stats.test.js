// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { useWebrtcStats } from './components/console/useWebrtcStats'

afterEach(() => vi.useRealTimers())
describe('selected preview statistics isolation', () => {
  it('does not apply an old peer stats reply to a newly selected target', async () => {
    vi.useFakeTimers()
    let resolveOld
    const oldPeer = { getStats: vi.fn(() => new Promise(resolve => { resolveOld = resolve })) }
    const newPeer = { getStats: vi.fn(async () => new Map()) }
    let peer = oldPeer
    const fps = ref(0), delay = ref(0), bitrate = ref('—')
    const controller = useWebrtcStats({
      getPeerConnection: () => peer, connected: ref(true), videoElement: ref(null), fps, delay, bitrate,
      sendControl: vi.fn(), handleVideoSilence: vi.fn(), getVideoConnectTs: () => Date.now(), getLastDragInputAt: () => 0,
    })
    controller.startStats()
    await vi.advanceTimersByTimeAsync(1000)
    controller.stopStats(); controller.resetWatchdogs()
    peer = newPeer
    controller.startStats()
    resolveOld(new Map([['video', { type: 'inbound-rtp', kind: 'video', framesPerSecond: 60, bytesReceived: 10000 }]]))
    await Promise.resolve(); await Promise.resolve()
    expect(fps.value).toBe(0)
    expect(bitrate.value).toBe('—')
    await vi.advanceTimersByTimeAsync(1000)
    expect(newPeer.getStats).toHaveBeenCalledOnce()
    controller.stopStats()
  })
  it('ignores a stats result resolved after closing a preview', async () => {
    vi.useFakeTimers()
    let resolveStats
    const peer = { getStats: () => new Promise(resolve => { resolveStats = resolve }) }
    const fps = ref(0)
    const controller = useWebrtcStats({
      getPeerConnection: () => peer, connected: ref(true), videoElement: ref(null), fps, delay: ref(0), bitrate: ref('—'),
      sendControl: vi.fn(), handleVideoSilence: vi.fn(), getVideoConnectTs: () => Date.now(), getLastDragInputAt: () => 0,
    })
    controller.startStats()
    await vi.advanceTimersByTimeAsync(1000)
    controller.stopStats()
    resolveStats(new Map([['video', { type: 'inbound-rtp', kind: 'video', framesPerSecond: 30 }]]))
    await Promise.resolve(); await Promise.resolve()
    expect(fps.value).toBe(0)
  })
})
