// @vitest-environment happy-dom
import { afterEach, describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import VideoWorkbench from '../../plugins/gamer-video/ui/src/components/video/VideoWorkbench.vue'
import { videoApi } from '../../plugins/gamer-video/ui/src/components/video/videoApi'
import { requestStageMedia } from './components/console/useConsoleStage'
import { packageStore } from './package-store'
vi.mock('../../plugins/gamer-video/ui/src/components/video/videoApi', () => ({ videoApi: { listMedia: vi.fn(), recordingHistory: vi.fn() } }))
vi.mock('./components/console/useConsoleStage', () => ({ requestStageMedia: vi.fn() }))
const wrappers = []
afterEach(() => { wrappers.splice(0).forEach(w => w.unmount()); vi.clearAllMocks() })
function mountWorkbench() {
  videoApi.listMedia.mockResolvedValue([{ id: 'video-1', name: '视频一' }]); videoApi.recordingHistory.mockResolvedValue([])
  const wrapper = mount(VideoWorkbench, { global: { stubs: { MediaLibrary: true, VideoSamples: true } } })
  wrappers.push(wrapper); return wrapper
}
describe('independent video workspace', () => {
  it('opens and previews videos without a Package or project layer', async () => {
    packageStore.currentPackageId = null
    const wrapper = mountWorkbench(); await flushPromises()
    expect(wrapper.findAll('.tab-btn').map(button => button.text())).toEqual(['视频', '素材包'])
    expect(wrapper.text()).not.toContain('配置包'); expect(wrapper.text()).not.toContain('项目')
    wrapper.find('media-library-stub').element.dispatchEvent(new Event('click'))
    wrapper.findComponent({ name: 'MediaLibrary' }).vm.$emit('select', 'video-1')
    expect(requestStageMedia).toHaveBeenCalledWith('video-1')
    packageStore.currentPackageId = 'another'; await flushPromises()
    expect(videoApi.listMedia).toHaveBeenCalledTimes(1)
  })
  it('opens the sample maker for the exact selected recording', async () => {
    const wrapper = mountWorkbench(); await flushPromises()
    wrapper.findComponent({ name: 'MediaLibrary' }).vm.$emit('recording-selected', { id: 'record-1' })
    await flushPromises()
    expect(wrapper.find('[data-testid="workbench-tab-sample"]').classes()).toContain('active')
    expect(wrapper.find('video-samples-stub').attributes('recordingid')).toBe('record-1')
  })
})
