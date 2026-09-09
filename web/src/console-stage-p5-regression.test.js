// @vitest-environment happy-dom
/**
 * P5-STAGE 回归：左侧 Stage 是 live/media 的唯一媒体来源控制器。
 * 覆盖播放器命令、来源代次、旧响应丢弃、媒体只读输入门禁和直接舞台控件。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('./api', async importOriginal => {
  const actual = await importOriginal()
  return {
    api: {
      ...actual.api,
      listMedia: vi.fn(async () => []),
      getMedia: vi.fn(async id => ({
        id,
        name: `${id}.mp4`,
        duration_us: 10000000,
        width: 1280,
        height: 720,
      })),
      activeRecording: vi.fn(async () => null),
    },
  }
})

vi.mock('./components/video/videoApi', () => ({
  videoApi: {
    mediaFrames: vi.fn(async () => ({ current: { index: 0, pts_us: 0 } })),
    mediaFrameNeighbors: vi.fn(async () => ({ prev: null, next: null })),
  },
}))

import { api } from './api'
import { useConsoleStage } from './components/console/useConsoleStage'
import ConsoleVideoStage from './components/console/ConsoleVideoStage.vue'
import { videoApi } from './components/video/videoApi'

function fakeMediaVideo({ width = 1280, height = 720, duration = 10 } = {}) {
  const listeners = new Map()
  return {
    videoWidth: width,
    videoHeight: height,
    duration,
    currentTime: 0,
    paused: true,
    playbackRate: 1,
    addEventListener(name, fn) { listeners.set(name, fn) },
    removeEventListener(name) { listeners.delete(name) },
    emit(name) { listeners.get(name)?.() },
    play() { this.paused = false; this.emit('play'); return Promise.resolve() },
    pause() { this.paused = true; this.emit('pause') },
  }
}

function mountController({ connectedValue = true, loadImage } = {}) {
  let controller
  const connected = ref(connectedValue)
  const toast = vi.fn()
  const Host = defineComponent({
    setup() {
      controller = useConsoleStage({
        toast,
        deviceId: () => 'device-a',
        connected,
        liveVideoEl: () => ({ videoWidth: 1920, videoHeight: 1080 }),
        loadImage,
      })
      return () => h('div')
    },
  })
  const wrapper = mount(Host)
  return { controller, connected, toast, wrapper }
}

async function openMedia(controller, id = 'm1') {
  api.getMedia.mockResolvedValueOnce({
    id,
    name: `${id}.mp4`,
    duration_us: 10000000,
    width: 1280,
    height: 720,
  })
  await controller.view.onMediaPick(id)
  const video = fakeMediaVideo()
  controller.attachMediaVideo(video)
  return video
}

beforeEach(() => {
  vi.clearAllMocks()
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('P5-STAGE：统一来源播放器与输入边界', () => {
  it('媒体模式拒绝 tap/swipe/key/text/Keymap 真机输入，非设备 UI 控制仍放行', async () => {
    const { controller, toast, wrapper } = mountController()
    await openMedia(controller)

    for (const type of ['tap', 'touch', 'swipe', 'key', 'text', 'input_event', 'keymap', 'mapping']) {
      expect(controller.guardDeviceInput({ type }), `应拒绝 ${type}`).toBe(false)
    }
    expect(controller.guardDeviceInput({ type: 'audio', on: true })).toBe(true)
    expect(controller.guardDeviceInput('tap')).toBe(false)
    expect(toast).toHaveBeenCalledTimes(1)

    controller.view.backToLive()
    expect(controller.view.canDeviceInput).toBe(true)
    expect(controller.guardDeviceInput({ type: 'tap' })).toBe(true)
    wrapper.unmount()
  })

  it('手动 seek、确定帧跳转和逐帧共享同一 Stage 时钟，回实时清除旧制作状态', async () => {
    const { controller, wrapper } = mountController()
    const video = await openMedia(controller)

    expect(controller.view.stageReady).toBe(true)
    expect(controller.view.seek(3.25)).toBe(true)
    expect(video.currentTime).toBe(3.25)
    expect(controller.view.frameAt).toEqual({ mediaId: 'm1', ptsUs: 3250000, index: null })

    expect(controller.view.jumpToMarker({ frame: { media_id: 'm1', frame_index: 7, pts_us: 3250000 } })).toBe(true)
    expect(video.currentTime).toBe(3.25)
    expect(controller.view.frameAt).toEqual({ mediaId: 'm1', ptsUs: 3250000, index: 7 })
    video.emit('seeked')
    expect(controller.view.frameAt.index).toBe(7)

    videoApi.mediaFrameNeighbors.mockResolvedValueOnce({
      index: 7,
      prev: { index: 6, pts_us: 3200000 },
      next: { index: 8, pts_us: 3300000 },
    })
    video.paused = false
    await controller.view.stepFrames(1)
    expect(video.paused).toBe(true)
    expect(video.currentTime).toBe(3.3)
    expect(controller.view.frameAt.index).toBe(8)

    controller.view.backToLive()
    expect(controller.view.kind).toBe('live')
    expect(controller.view.frameAt).toBeNull()
    expect(controller.view.playing).toBe(false)
    expect(video.paused).toBe(true)
    wrapper.unmount()
  })

  it('媒体切换期间返回实时：旧媒体详情响应不能重新进入媒体模式', async () => {
    let resolveOld
    api.getMedia.mockReturnValueOnce(new Promise(resolve => { resolveOld = resolve }))
    const { controller, wrapper } = mountController()

    const pending = controller.view.onMediaPick('old')
    expect(controller.view.kind).toBe('media')
    controller.view.backToLive()
    resolveOld({ id: 'old', name: 'old.mp4', duration_us: 1000000, width: 640, height: 360 })
    await pending
    await flushPromises()

    expect(controller.view.kind).toBe('live')
    expect(controller.view.mediaId).toBe('')
    expect(controller.view.canDeviceInput).toBe(true)
    wrapper.unmount()
  })

  it('指定帧图片加载期间切换来源：旧 captureFrame 结果丢弃', async () => {
    let resolveImage
    const { controller, wrapper } = mountController({
      loadImage: () => new Promise(resolve => { resolveImage = resolve }),
    })
    await openMedia(controller)
    const pending = controller.captureFrame()
    controller.view.backToLive()
    resolveImage({ naturalWidth: 640, naturalHeight: 360 })

    await expect(pending).resolves.toBeNull()
    wrapper.unmount()
  })

  it('逐帧请求返回后来源已切换：旧邻帧响应不改变实时 Stage', async () => {
    let resolveNeighbors
    videoApi.mediaFrameNeighbors.mockReturnValueOnce(new Promise(resolve => { resolveNeighbors = resolve }))
    const { controller, wrapper } = mountController()
    const video = await openMedia(controller)
    const pending = controller.view.stepFrames(1)
    controller.view.backToLive()
    resolveNeighbors({ index: 0, prev: null, next: { index: 1, pts_us: 33333 } })

    await pending
    expect(controller.view.kind).toBe('live')
    expect(video.currentTime).toBe(0)
    expect(controller.view.frameAt).toBeNull()
    wrapper.unmount()
  })
})

describe('P5-STAGE：直接舞台组件使用同一控制器时钟', () => {
  it('媒体视频只读且 seek 控件回调 Stage.seek，不创建第二个播放状态', async () => {
    const stage = reactive({
      kind: 'media',
      mediaId: 'm1',
      mediaSrc: '/api/media/m1/file',
      mediaName: 'clip.mp4',
      mediaOptions: [{ id: 'm1', name: 'clip.mp4' }],
      mediaSizeLabel: '1280×720',
      stageReady: true,
      currentTime: 1,
      duration: 10,
      playing: false,
      timeText: '00:01.0',
      durationText: '00:10.0',
      rate: 1,
      rateOptions: [1],
      seek: vi.fn(),
      togglePlay: vi.fn(),
      stepFrames: vi.fn(),
      setRate: vi.fn(),
      onMediaPick: vi.fn(),
      backToLive: vi.fn(),
    })
    const noop = vi.fn()
    const wrapper = mount(ConsoleVideoStage, {
      props: {
        stage,
        scriptFx: { tap: { show: false }, swipe: { show: false }, hit: { show: false } },
        loupe: { show: false, x: 0, y: 0, zoom: 2 },
        onMouseDown: noop,
        onMouseMove: noop,
        onMouseUp: noop,
        onWheel: noop,
        onVideoMouseLeave: noop,
        flushAndConnect: noop,
        fullscreen: noop,
      },
    })

    const media = wrapper.find('.media-stream')
    expect(media.attributes('tabindex')).toBe('-1')
    expect(media.attributes('aria-readonly')).toBe('true')
    const seek = wrapper.find('.mc-seek')
    await seek.setValue('4.5')
    expect(stage.seek).toHaveBeenCalledWith('4.5')
    expect(wrapper.find('.media-mode-badge').text()).toContain('只读')
    wrapper.unmount()
  })
})
