// @vitest-environment happy-dom
/**
 * P5-VWORKFLOW：时间轴真实帧定位、校准输入和 TemplateStudio 冻结帧联动。
 * 只替换网络/API 与 canvas 外部环境，不绕过组件之间的帧身份传递。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('./api', () => ({
  api: { listTemplates: vi.fn() },
}))

vi.mock('./components/video/videoApi', async importOriginal => {
  const actual = await importOriginal()
  return {
    ...actual,
    videoApi: {
      ...actual.videoApi,
      mediaFrames: vi.fn(),
      mediaFrameNeighbors: vi.fn(),
      createTemplateFromFrame: vi.fn(),
      visionTestTemplate: vi.fn(),
    },
  }
})

import VideoTimeline from './components/video/VideoTimeline.vue'
import TemplateStudio from './components/video/TemplateStudio.vue'
import { api } from './api'
import { videoApi } from './components/video/videoApi'

const MEDIA_A = { id: 'media-a', name: 'A.mp4', duration_us: 2_000_000, width: 100, height: 50 }
const MEDIA_B = { id: 'media-b', name: 'B.mp4', duration_us: 3_000_000, width: 200, height: 100 }
const CAL_V1 = {
  version: 1,
  rotation: 0,
  pixel_aspect: { num: 1, den: 1 },
  content_rect: null,
  reference_size: { width: 100, height: 50 },
}

function frameTable(mediaId, frameCount = 5) {
  return {
    media_id: mediaId,
    frame_count: frameCount,
    first_pts_us: 0,
    last_pts_us: 1_600_000,
  }
}

function deferred() {
  let resolve
  let reject
  const promise = new Promise((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}

function makeCanvasStubs() {
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockImplementation(() => ({
    imageSmoothingEnabled: false,
    drawImage: vi.fn(),
  }))
  vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation(callback => {
    callback(new Blob([new Uint8Array([1])], { type: 'image/png' }))
  })
  class Reader {
    result = 'data:image/png;base64,AAAA'
    readAsDataURL() { queueMicrotask(() => this.onload?.()) }
  }
  vi.stubGlobal('FileReader', Reader)
}

beforeEach(() => {
  vi.clearAllMocks()
  api.listTemplates.mockResolvedValue([])
  videoApi.mediaFrames.mockResolvedValue(frameTable('media-a'))
  videoApi.mediaFrameNeighbors.mockResolvedValue({
    media_id: 'media-a',
    index: 3,
    pts_us: 975_000,
    prev: { index: 2, pts_us: 700_000 },
    next: { index: 4, pts_us: 1_200_000 },
  })
})

describe('P5-VWORKFLOW 时间轴真实帧工作流', () => {
  it('指定帧只提交 index，展示 PTS 使用服务端邻帧响应', async () => {
    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [] },
    })
    await flushPromises()

    await wrapper.find('[data-testid="frame-index-input"]').setValue('3')
    await wrapper.find('[data-testid="frame-index-go"]').trigger('click')
    await flushPromises()

    expect(videoApi.mediaFrameNeighbors).toHaveBeenCalledWith('media-a', 3)
    expect(wrapper.find('[data-testid="frame-image"]').attributes('src'))
      .toBe('/api/media/media-a/frame?index=3&max_width=640')
    expect(wrapper.text()).toContain('pts_us=975000')
    await wrapper.find('[data-testid="frame-image"]').trigger('load')

    await wrapper.find('[data-testid="frame-index-input"]').setValue('5')
    await wrapper.find('[data-testid="frame-index-go"]').trigger('click')
    expect(wrapper.find('[data-testid="frame-error"]').text()).toContain('索引越界')
    expect(videoApi.mediaFrameNeighbors).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('空帧表明确禁用逐帧、指定帧和精确帧操作', async () => {
    videoApi.mediaFrames.mockResolvedValueOnce(frameTable('media-a', 0))
    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [] },
    })
    await flushPromises()

    expect(wrapper.find('[data-testid="frame-empty"]').text()).toContain('没有可用的展示帧')
    expect(wrapper.find('[data-testid="frame-prev"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="frame-index-go"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="frame-exact"]').attributes('disabled')).toBeDefined()
    expect(wrapper.find('[data-testid="marker-add"]').attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('校准表单拒绝非法比例和部分有效区域，不静默钳制后保存', async () => {
    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [] },
    })
    await flushPromises()

    await wrapper.find('[data-testid="calibration-pa-num"]').setValue('0')
    await wrapper.find('[data-testid="calibration-apply"]').trigger('click')
    expect(wrapper.find('[data-testid="calibration-error"]').text()).toContain('像素比例分子')
    expect(wrapper.emitted('save-calibration')).toBeUndefined()

    await wrapper.find('[data-testid="calibration-pa-num"]').setValue('1')
    await wrapper.find('[data-testid="calibration-rect-w"]').setValue('30')
    await wrapper.find('[data-testid="calibration-apply"]').trigger('click')
    expect(wrapper.find('[data-testid="calibration-error"]').text()).toContain('有效区域 x')
    expect(wrapper.emitted('save-calibration')).toBeUndefined()
    wrapper.unmount()
  })
})

describe('P5-VWORKFLOW TemplateStudio 冻结帧', () => {
  it('打开时冻结媒体/帧/Package/校准，保存只使用冻结确定帧且不重新抓帧', async () => {
    makeCanvasStubs()
    videoApi.createTemplateFromFrame.mockResolvedValue({
      name: '按钮#100_100_600_600.png', short_name: '按钮.png', size: 1,
    })
    const wrapper = mount(TemplateStudio, {
      props: {
        open: true,
        media: MEDIA_A,
        frame: { frameIndex: 4, ptsUs: 400_000 },
        calibration: CAL_V1,
        packageId: 'pkg-a',
        yamlReady: true,
      },
    })
    await flushPromises()

    const image = wrapper.find('[data-testid="studio-frame-image"]')
    expect(image.attributes('src')).toBe('/api/media/media-a/frame?index=4')
    Object.defineProperty(image.element, 'naturalWidth', { value: 100, configurable: true })
    Object.defineProperty(image.element, 'naturalHeight', { value: 50, configurable: true })
    image.element.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 50 })
    await image.trigger('load')

    await image.trigger('mousedown', { button: 0, clientX: 10, clientY: 5 })
    await image.trigger('mousemove', { clientX: 60, clientY: 30 })
    await image.trigger('mouseup')
    expect(wrapper.find('[data-testid="studio-save"]').attributes('disabled')).toBeUndefined()

    await wrapper.setProps({
      media: MEDIA_B,
      frame: { frameIndex: 1, ptsUs: 100_000 },
      calibration: { ...CAL_V1, version: 2, rotation: 90 },
      packageId: 'pkg-b',
    })
    await flushPromises()
    expect(wrapper.find('[data-testid="studio-frame-image"]').attributes('src'))
      .toBe('/api/media/media-a/frame?index=4')

    await wrapper.find('[data-testid="studio-template-name"]').setValue('按钮')
    await wrapper.find('[data-testid="studio-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.createTemplateFromFrame).toHaveBeenCalledWith(expect.objectContaining({
      packageId: 'pkg-a',
      frame: { media_id: 'media-a', frame_index: 4, pts_us: 400_000 },
      calibration: expect.objectContaining({ version: 1, rotation: 0 }),
    }))
    expect(videoApi.mediaFrames).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('关闭/重开后，旧离线匹配响应不能覆盖新的制作会话', async () => {
    const pending = deferred()
    videoApi.visionTestTemplate.mockReturnValueOnce(pending.promise)
    const wrapper = mount(TemplateStudio, {
      props: {
        open: true,
        media: MEDIA_A,
        frame: { frameIndex: 2, ptsUs: 200_000 },
        calibration: CAL_V1,
        packageId: 'pkg-a',
        yamlReady: true,
      },
    })
    await flushPromises()
    const image = wrapper.find('[data-testid="studio-frame-image"]')
    Object.defineProperty(image.element, 'naturalWidth', { value: 100, configurable: true })
    Object.defineProperty(image.element, 'naturalHeight', { value: 50, configurable: true })
    image.element.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 50 })
    await image.trigger('load')
    await wrapper.find('[data-testid="studio-test-name"]').setValue('按钮')
    await wrapper.find('[data-testid="studio-test"]').trigger('click')

    await wrapper.setProps({ open: false })
    await wrapper.setProps({
      open: true,
      media: MEDIA_B,
      frame: { frameIndex: 1, ptsUs: 100_000 },
      packageId: 'pkg-b',
    })
    pending.resolve({ hit: true, x: 1, y: 2, width: 3, height: 4, score: 0.99 })
    await flushPromises()

    expect(wrapper.find('[data-testid="studio-result"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="studio-frame-image"]').attributes('src'))
      .toBe('/api/media/media-b/frame?index=1')
    wrapper.unmount()
  })
})
