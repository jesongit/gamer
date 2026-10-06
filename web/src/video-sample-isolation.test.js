// @vitest-environment happy-dom
import { mount, flushPromises } from '@vue/test-utils'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import VideoSamples from '../../plugins/gamer-video/ui/src/components/video/VideoSamples.vue'
import { videoApi } from '../../plugins/gamer-video/ui/src/components/video/videoApi'
vi.mock('../../plugins/gamer-video/ui/src/components/video/videoApi', () => ({ videoApi: { listSamples: vi.fn(), recordingStatus: vi.fn(), recordingEvents: vi.fn(), mediaFrames: vi.fn(), mediaFrameNeighbors: vi.fn(), mediaFrameUrl: vi.fn(), createSample: vi.fn(), sampleUrl: vi.fn(), importSample: vi.fn() } }))
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b }); return { promise, resolve, reject } }
const record = id => ({ id, state: 'completed', event_count: 1, segments: [{ media_id: 'm1', start_us: 0, duration_us: 1000000 }] })
const mountSample = () => mount(VideoSamples, { props: { packageId: 'pkg', recordingId: 'r1', recordings: [record('r1'), record('r2')] } })
beforeEach(() => {
  vi.clearAllMocks(); videoApi.listSamples.mockResolvedValue([]); videoApi.recordingStatus.mockImplementation(async id => record(id)); videoApi.recordingEvents.mockResolvedValue([{ event_id: 'e1' }]); videoApi.sampleUrl.mockReturnValue('/sample')
  videoApi.mediaFrames.mockImplementation(async (id, q) => ({ frame_count: 3, current: { index: q.ptsUs === 0 ? 0 : 2, pts_us: q.ptsUs } })); videoApi.mediaFrameUrl.mockReturnValue('/frame')
})
async function confirm(w) {
  await flushPromises(); await w.findAll('button').find(b => b.text() === '查看 START / END').trigger('click'); await flushPromises()
  for (const img of w.findAll('img')) await img.trigger('load')
  await w.get('textarea').setValue('reward claimed'); await w.get('[data-testid=sample-goal-confirmed]').setValue(true)
}
describe('sample context and async isolation', () => {
  it('ignores stale recording events after switching recording', async () => {
    const old = deferred(); videoApi.recordingEvents.mockImplementation(id => id === 'r1' ? old.promise : Promise.resolve([{ event_id: 'new' }, { event_id: 'new2' }]))
    const w = mountSample(); await flushPromises(); await w.setProps({ recordingId: 'r2' }); await flushPromises(); old.resolve([{ event_id: 'old' }]); await flushPromises()
    expect(w.text()).toContain('2 个已记录操作'); expect(w.get('[data-testid=sample-recording]').element.value).toBe('r2'); w.unmount()
  })
  it('ignores stale sample lists after switching package', async () => {
    const old = deferred(); videoApi.listSamples.mockImplementation(pkg => pkg === 'pkg' ? old.promise : Promise.resolve([{ id: 'new', name: 'New sample', status: 'complete', path: 'new' }]))
    const w = mountSample(); await flushPromises(); await w.setProps({ packageId: 'newpkg' }); await flushPromises(); old.resolve([{ id: 'old', name: 'Old sample', path: 'old' }]); await flushPromises()
    expect(w.text()).toContain('New sample'); expect(w.text()).not.toContain('Old sample'); w.unmount()
  })
  it('does not apply stale create results or duplicate pending submissions', async () => {
    const pending = deferred(); videoApi.createSample.mockReturnValue(pending.promise)
    const w = mountSample(); await confirm(w); await w.get('[data-testid=sample-create]').trigger('click'); await w.get('[data-testid=sample-create]').trigger('click')
    expect(videoApi.createSample).toHaveBeenCalledTimes(1)
    await w.setProps({ packageId: 'newpkg' }); await flushPromises(); pending.resolve({ manifest: { name: 'Old result', status: 'complete' } }); await flushPromises()
    expect(w.find('[data-testid=sample-result]').exists()).toBe(false); w.unmount()
  })
  it('clears confirmation when goal, boundary, recording, or frame availability changes', async () => {
    const w = mountSample(); await confirm(w)
    await w.get('textarea').setValue('different goal'); expect(w.get('[data-testid=sample-goal-confirmed]').element.checked).toBe(false)
    await w.get('[data-testid=sample-goal-confirmed]').setValue(true); await w.findAll('img')[1].trigger('error'); expect(w.get('[data-testid=sample-create]').attributes('disabled')).toBeDefined()
    await w.setProps({ recordingId: 'r2' }); await flushPromises(); expect(w.find('[data-testid=sample-boundaries]').exists()).toBe(false); w.unmount()
  })
  it('shows failed creation and keeps reviewable inputs for retry', async () => {
    videoApi.createSample.mockRejectedValueOnce(new Error('disk full')).mockResolvedValueOnce({ manifest: { name: 'retry', status: 'unable_to_validate', diagnostics: [{ code: 'window.insufficient', message: 'more evidence needed' }] } })
    const w = mountSample(); await confirm(w); await w.get('[data-testid=sample-create]').trigger('click'); await flushPromises()
    expect(w.get('[data-testid=sample-error]').text()).toContain('disk full'); expect(w.get('textarea').element.value).toBe('reward claimed')
    await w.get('[data-testid=sample-create]').trigger('click'); await flushPromises(); expect(w.get('[data-testid=sample-result]').text()).toContain('无法验证'); expect(videoApi.createSample).toHaveBeenCalledTimes(2); w.unmount()
  })
  it('does not hide missing event evidence or equate it with an empty valid sample', async () => {
    videoApi.recordingEvents.mockRejectedValue(new Error('recording_events_corrupt'))
    const w = mountSample(); await flushPromises(); expect(w.get('[data-testid=sample-error]').text()).toContain('recording_events_corrupt'); expect(w.get('[data-testid=sample-create]').attributes('disabled')).toBeDefined(); w.unmount()
  })
})
