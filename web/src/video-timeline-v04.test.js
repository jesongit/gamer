// @vitest-environment happy-dom
/**
 * VideoTimeline V04 专属回归：预览时钟与服务端确定制作帧隔离。
 * 只 mock 时间轴所需的 videoApi，不依赖真实媒体、ffmpeg 或设备。
 */
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('./components/video/videoApi', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    videoApi: {
      ...actual.videoApi,
      mediaFrames: vi.fn(),
      mediaFrameNeighbors: vi.fn(),
    },
  }
})

import VideoTimeline from './components/video/VideoTimeline.vue'
import { videoApi } from './components/video/videoApi'

const MEDIA_A = { id: 'media-a', name: 'A.mp4', duration_us: 5_000_000, width: 1920, height: 1080 }
const MEDIA_B = { id: 'media-b', name: 'B.mp4', duration_us: 7_000_000, width: 1280, height: 720 }
const CAL_V1 = { version: 1, rotation: 0, pixel_aspect: { num: 1, den: 1 }, content_rect: null, reference_size: { width: 1920, height: 1080 } }

function frameTable(mediaId, position, frameCount = 20) {
  return {
    media_id: mediaId,
    frame_count: frameCount,
    first_pts_us: 0,
    last_pts_us: 2_000_000,
    ...(position ? { current: position } : {}),
  }
}

function deferred() {
  let resolve
  let reject
  const promise = new Promise((res, rej) => { resolve = res; reject = rej })
  return { promise, resolve, reject }
}

beforeEach(() => {
  vi.clearAllMocks()
  videoApi.mediaFrames.mockResolvedValue(frameTable('media-a', null))
  videoApi.mediaFrameNeighbors.mockResolvedValue({
    media_id: 'media-a',
    index: 4,
    pts_us: 400_000,
    prev: { index: 3, pts_us: 300_000 },
    next: { index: 5, pts_us: 500_000 },
  })
})

describe('VideoTimeline V04 确定帧隔离', () => {
  it('程序锁帧触发的 seek 回显不清锁，用户 seek 后制作操作重新解析新帧', async () => {
    videoApi.mediaFrames
      .mockResolvedValueOnce(frameTable('media-a', null))
      .mockResolvedValueOnce(frameTable('media-a', { index: 4, pts_us: 400_000 }))
      .mockResolvedValueOnce(frameTable('media-a', { index: 9, pts_us: 900_000 }))

    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [], yamlReady: true },
    })
    await flushPromises()

    await wrapper.find('[data-testid="frame-exact"]').trigger('click')
    await flushPromises()
    const image = wrapper.find('[data-testid="frame-image"]')
    expect(image.attributes('src')).toBe('/api/media/media-a/frame?index=4&max_width=640')
    await image.trigger('load')

    // 相邻帧会产生新的 PNG 请求；旧请求稍后报错也不能清掉新帧。
    const oldImageElement = image.element
    await wrapper.find('[data-testid="frame-next"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-image"]').attributes('src')).toBe('/api/media/media-a/frame?index=5&max_width=640')
    oldImageElement.dispatchEvent(new Event('error'))
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-image"]').attributes('src')).toBe('/api/media/media-a/frame?index=5&max_width=640')
    await wrapper.find('[data-testid="frame-image"]').trigger('load')

    // showFrameByIndex 会把预览 seek 到锁帧时刻；seeked/timeupdate 回显不能误判为用户移动。
    const video = wrapper.find('[data-testid="video-preview"]')
    video.element.currentTime = 0.5
    await video.trigger('seeking')
    await video.trigger('timeupdate')
    await video.trigger('seeked')
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="frame-to-template"]').attributes('disabled')).toBeUndefined()

    // 用户拖动后旧制作帧立即失效；加标记必须按新预览位置重新向服务端解析。
    video.element.currentTime = 0.9
    await video.trigger('seeking')
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(false)

    await wrapper.find('[data-testid="marker-add"]').trigger('click')
    await flushPromises()
    expect(wrapper.emitted('add-marker')?.[0]?.[0]).toEqual({
      label: '',
      frame: { media_id: 'media-a', frame_index: 9, pts_us: 900_000, calibration_version: 1 },
    })
    wrapper.unmount()
  })

  it('播放会使制作帧失效，素材切换后旧帧表和旧 PNG 响应均不串入新素材', async () => {
    const oldTable = deferred()
    const newTable = deferred()
    videoApi.mediaFrames.mockImplementation((mediaId) => (
      mediaId === 'media-a' ? oldTable.promise : newTable.promise
    ))

    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [], yamlReady: true },
    })
    await flushPromises()

    // 先切换，确保 A 的挂起响应返回时不能覆盖 B。
    await wrapper.setProps({ media: MEDIA_B })
    newTable.resolve(frameTable('media-b', null, 31))
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-count"]').text()).toContain('31 帧')

    oldTable.resolve(frameTable('media-a', { index: 2, pts_us: 200_000 }, 99))
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-count"]').text()).toContain('31 帧')
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(false)

    // B 上锁定一帧，然后由主动播放清除制作帧。
    videoApi.mediaFrames.mockResolvedValueOnce(frameTable('media-b', { index: 6, pts_us: 600_000 }))
    await wrapper.find('[data-testid="frame-exact"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="frame-image"]').trigger('load')
    await wrapper.find('[data-testid="video-preview"]').trigger('play')
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('校准变化或请求代次变化后，旧帧响应不能恢复制作入口', async () => {
    const pending = deferred()
    videoApi.mediaFrames
      .mockResolvedValueOnce(frameTable('media-a', null))
      .mockReturnValueOnce(pending.promise)

    const wrapper = mount(VideoTimeline, {
      props: { media: MEDIA_A, calibration: CAL_V1, markers: [], yamlReady: true },
    })
    await flushPromises()
    await wrapper.find('[data-testid="frame-exact"]').trigger('click')

    // 让正在解析的 v1 请求失效，再返回一个看似合法的 media-a 帧。
    await wrapper.setProps({ calibration: { ...CAL_V1, version: 2 } })
    pending.resolve(frameTable('media-a', { index: 8, pts_us: 800_000 }))
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="frame-to-template"]').exists()).toBe(false)

    // 响应媒体 ID 不符时也不能建立锁帧。
    videoApi.mediaFrames.mockResolvedValueOnce(frameTable('wrong-media', { index: 1, pts_us: 100_000 }))
    await wrapper.find('[data-testid="frame-exact"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="frame-box"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="frame-error"]').text()).toContain('当前素材不一致')
    wrapper.unmount()
  })
})
