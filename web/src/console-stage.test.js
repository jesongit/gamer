// @vitest-environment happy-dom
/**
 * 统一舞台 StageSource（视频工作台 V1，实施合同 §6 / 计划 §4.2）：
 * - api.js 媒体/录制合同方法端点形态（合同 §1/§2/§5）；
 * - StageSource 状态机：kind/generation/canDeviceInput 切换语义；
 * - 输入安全红线：媒体模式下舞台产生的设备输入在路由门禁统一拒绝；
 * - 指定帧：frameAt/captureFrame 走服务端确定帧；来源切换后过期结果不应用；
 * - TemplateCropModal 数据流：裁切底图 = 指定帧（媒体 = 帧图像，绝不在保存时
 *   重抓最新设备画面），坐标系用当前舞台来源的 displaySize。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'

// ---------- fetch 桩（端点形态断言用真实 api 模块，见 vi.importActual） ----------
function jsonRes(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: name => (/^content-type$/i.test(name) ? 'application/json' : null) },
    json: async () => body,
  }
}

// ---------- 组合式测试用 api mock（只换网络方法，URL 构造保持真实实现） ----------
vi.mock('./api', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    api: {
      ...actual.api,
      listMedia: vi.fn(async () => []),
      getMedia: vi.fn(async id => ({ id, name: `${id}.mp4`, duration_us: 10000000, width: 1920, height: 1080 })),
      recordingStart: vi.fn(async () => ({ id: 'r-new', state: 'recording' })),
      recordingStop: vi.fn(async () => ({ id: 'r-new', state: 'completed', segments: [] })),
      activeRecording: vi.fn(async () => null),
    },
  }
})

import { api } from './api'
import { useConsoleStage, formatStageClock } from './components/console/useConsoleStage'
import { useConsoleTemplates } from './components/console/useConsoleTemplates'

const { api: realApi } = await vi.importActual('./api')

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('fetch', vi.fn())
})

afterEach(() => {
  vi.unstubAllGlobals()
  document.createElement = origCreateElement
})

// happy-dom 的 canvas getContext 返回 null：裁切底图冻结/预览用桩画布替身
const origCreateElement = document.createElement.bind(document)
function fakeCanvas2d() {
  return {
    drawImage: vi.fn(), clearRect: vi.fn(), fillRect: vi.fn(), strokeRect: vi.fn(),
    beginPath: vi.fn(), moveTo: vi.fn(), lineTo: vi.fn(), stroke: vi.fn(), fill: vi.fn(),
    arc: vi.fn(), fillText: vi.fn(), set fillStyle(_) {}, set strokeStyle(_) {},
    set lineWidth(_) {}, set font(_) {}, set imageSmoothingEnabled(_) {},
    getImageData: () => ({ data: [16, 32, 48, 255] }),
  }
}
function stubCanvasCreation() {
  document.createElement = (tag) => {
    if (String(tag).toLowerCase() === 'canvas') {
      const ctx = fakeCanvas2d()
      return { width: 0, height: 0, style: {}, getContext: () => ctx, toDataURL: () => 'data:image/png;base64,QUJD' }
    }
    return origCreateElement(tag)
  }
}

// ---------- 媒体/录制 REST（合同 §1/§2/§5；走真实 api 封装） ----------
describe('api.js 媒体与录制合同方法', () => {
  it('listMedia → GET /api/media，解包 media 数组', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { media: [{ id: 'm1', name: 'a.mp4' }] }))
    await expect(realApi.listMedia()).resolves.toEqual([{ id: 'm1', name: 'a.mp4' }])
    expect(fetch.mock.calls[0][0]).toBe('/api/media')
    expect(fetch.mock.calls[0][1].method).toBe('GET')
  })

  it('getMedia/deleteMedia 寻址 = media id（URL 编码）', async () => {
    fetch.mockResolvedValue(jsonRes(200, { id: 'm 1' }))
    await realApi.getMedia('m 1')
    expect(fetch.mock.calls[0][0]).toBe('/api/media/m%201')

    fetch.mockResolvedValue(jsonRes(204, null))
    await realApi.deleteMedia('m1')
    expect(fetch.mock.calls[1][0]).toBe('/api/media/m1')
    expect(fetch.mock.calls[1][1].method).toBe('DELETE')
  })

  it('importMedia → POST /api/media/import?name=<urlencoded>，raw 字节 + octet-stream', async () => {
    fetch.mockResolvedValueOnce(jsonRes(201, { id: 'm2', name: 'clip.mp4' }))
    const bytes = new Uint8Array([1, 2, 3])
    await expect(realApi.importMedia(bytes, '我的 clip.mp4')).resolves.toEqual({ id: 'm2', name: 'clip.mp4' })
    const [url, options] = fetch.mock.calls[0]
    expect(url).toBe(`/api/media/import?name=${encodeURIComponent('我的 clip.mp4')}`)
    expect(options.method).toBe('POST')
    expect(options.headers['Content-Type']).toBe('application/octet-stream')
    expect(options.body).toBe(bytes)
  })

  it('mediaFileUrl/mediaFrameUrl 只拼 URL；frame 查询 = pts_us|index|max_width，pts_us 优先', () => {
    expect(fetch).not.toHaveBeenCalled()
    expect(realApi.mediaFileUrl('m1')).toBe('/api/media/m1/file')
    expect(realApi.mediaFrameUrl('m1', { ptsUs: 1234.6, maxWidth: 640 })).toBe('/api/media/m1/frame?pts_us=1235&max_width=640')
    expect(realApi.mediaFrameUrl('m1', { index: 5 })).toBe('/api/media/m1/frame?index=5')
    expect(realApi.mediaFrameUrl('m1', { ptsUs: 10, index: 5 })).toBe('/api/media/m1/frame?pts_us=10&index=5')
    expect(realApi.mediaFrameUrl('m1')).toBe('/api/media/m1/frame')
  })

  it('recordingStart/Stop/Cancel/Status 端点形态（合同 §2）', async () => {
    fetch.mockResolvedValue(jsonRes(200, { id: 'r-1', state: 'completed' }))
    await realApi.recordingStart('dev-a')
    expect(fetch.mock.calls[0][0]).toBe('/api/recording/start')
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ device_id: 'dev-a' })

    await realApi.recordingStop('r-1')
    expect(fetch.mock.calls[1][0]).toBe('/api/recording/r-1/stop')
    await realApi.recordingCancel('r-1')
    expect(fetch.mock.calls[2][0]).toBe('/api/recording/r-1/cancel')
    await realApi.recordingStatus('r-1')
    expect(fetch.mock.calls[3][0]).toBe('/api/recording/r-1')
    expect(fetch.mock.calls[3][1].method).toBe('GET')
  })

  it('activeRecording：200 → 会话；404（无活动会话，轮询常态）→ null 不抛错', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { id: 'r1', state: 'recording' }))
    await expect(realApi.activeRecording('dev-a')).resolves.toEqual({ id: 'r1', state: 'recording' })
    expect(fetch.mock.calls[0][0]).toBe('/api/recording/active?device_id=dev-a')

    fetch.mockResolvedValueOnce(jsonRes(404, { error: 'recording_not_found' }))
    await expect(realApi.activeRecording('dev-a')).resolves.toBeNull()
  })

  it('recordingEvents：解包 events 数组（时间轴升序由服务端保证）', async () => {
    fetch.mockResolvedValueOnce(jsonRes(200, { schema_version: 1, events: [{ event_id: 'e1' }] }))
    await expect(realApi.recordingEvents('r-1')).resolves.toEqual([{ event_id: 'e1' }])
    expect(fetch.mock.calls[0][0]).toBe('/api/recording/r-1/events')
  })

  it('createVideoDraft → POST 扩展 call 通路 action=automation.create_draft，兼容 {ok,data} 信封', async () => {
    const data = { yaml: 'version: 3\nsteps: []', diagnostics: [] }
    fetch.mockResolvedValueOnce(jsonRes(200, { ok: true, data }))
    await expect(realApi.createVideoDraft('r-1', ['e1', 'e2'])).resolves.toBe(data)
    const [url, options] = fetch.mock.calls[0]
    expect(url).toBe('/api/extensions/gamer.yaml/call')
    expect(options.method).toBe('POST')
    expect(JSON.parse(options.body)).toEqual({
      action: 'automation.create_draft',
      values: { recording_id: 'r-1', event_ids: ['e1', 'e2'] },
    })

    const bare = { yaml: 'version: 3\nsteps: []', diagnostics: [{ event_id: 'e9', reason: 'multi_touch' }] }
    fetch.mockResolvedValueOnce(jsonRes(200, bare))
    await expect(realApi.createVideoDraft('r-1', [])).resolves.toEqual(bare)
  })
})

// ---------- StageSource 状态机与输入门禁 ----------
function fakeMediaVideoEl({ width = 1280, height = 720, duration = 10, currentTime = 0 } = {}) {
  const listeners = new Map()
  return {
    videoWidth: width,
    videoHeight: height,
    currentTime,
    duration,
    paused: true,
    playbackRate: 1,
    addEventListener: (n, f) => listeners.set(n, f),
    removeEventListener: (n, f) => listeners.delete(n),
    emit(name) { listeners.get(name)?.() },
    play() { this.paused = false; this.emit('play'); return Promise.resolve() },
    pause() { this.paused = true; this.emit('pause') },
  }
}

function mountStage({ connectedValue = true, liveVideo = null, loadImage } = {}) {
  let ctl
  const connected = ref(connectedValue)
  const Host = defineComponent({
    setup() {
      ctl = useConsoleStage({
        toast: vi.fn(),
        deviceId: () => 'dev-a',
        connected,
        liveVideoEl: () => liveVideo || { videoWidth: 1920, videoHeight: 1080 },
        loadImage: loadImage || (async () => ({ naturalWidth: 640, naturalHeight: 360 })),
      })
      return () => h('div')
    },
  })
  const wrapper = mount(Host)
  return { ctl, wrapper, connected }
}

describe('useConsoleStage：StageSource 状态机', () => {
  it('初始为实时来源：generation 0、canDeviceInput=true、frameAt 为空', () => {
    const { ctl, wrapper } = mountStage()
    expect(ctl.view.kind).toBe('live')
    expect(ctl.view.sourceId).toBe('')
    expect(ctl.view.generation).toBe(0)
    expect(ctl.view.canDeviceInput).toBe(true)
    expect(ctl.view.stageReady).toBe(true)
    expect(ctl.frameAt()).toBeNull()
    wrapper.unmount()
  })

  it('切换视频来源：generation 递增、自动选中最新素材、canDeviceInput=false；切回实时恢复', async () => {
    api.listMedia.mockResolvedValue([{ id: 'm1', name: 'a.mp4', duration_us: 5000000, width: 1280, height: 720 }])
    const { ctl, wrapper } = mountStage()
    const g0 = ctl.view.generation
    ctl.view.toggleKind()
    await flushPromises()
    expect(ctl.view.kind).toBe('media')
    expect(ctl.view.generation).toBeGreaterThan(g0)
    expect(ctl.view.canDeviceInput).toBe(false)
    expect(ctl.view.mediaId).toBe('m1')
    expect(api.listMedia).toHaveBeenCalled()
    expect(ctl.view.mediaSrc).toBe('/api/media/m1/file')
    expect(ctl.view.stageReady).toBe(false) // 媒体画面未就绪前不可框选

    ctl.view.backToLive()
    await flushPromises()
    expect(ctl.view.kind).toBe('live')
    expect(ctl.view.canDeviceInput).toBe(true)
    wrapper.unmount()
  })

  it('媒体库为空进入视频来源：提示且无素材选中（不连接设备不触发 ADB）', async () => {
    api.listMedia.mockResolvedValue([])
    const { ctl, wrapper } = mountStage()
    ctl.view.toggleKind()
    await flushPromises()
    expect(ctl.view.kind).toBe('media')
    expect(ctl.view.mediaId).toBe('')
    expect(ctl.view.mediaSrc).toBe('')
    wrapper.unmount()
  })
})

describe('useConsoleStage：输入安全红线（媒体模式统一拒绝舞台设备输入）', () => {
  it('guardDeviceInput：媒体模式拒绝触控/滚轮/按键/文本/启停应用，切回实时放行；warn 只提示一次', async () => {
    const { ctl, wrapper } = mountStage()
    const inputs = [
      { type: 'touch', action: 'down' },
      { type: 'swipe' },
      { type: 'scroll' },
      { type: 'key', action: 0 },
      { type: 'input_event' },
      { type: 'text', text: 'a' },
      { type: 'start_app', app: 'com.x' },
      { type: 'stop_app', app: 'com.x' },
    ]
    for (const msg of inputs) expect(ctl.guardDeviceInput(msg)).toBe(true)

    ctl.view.toggleKind()
    await flushPromises()
    expect(ctl.view.canDeviceInput).toBe(false)
    for (const msg of inputs) {
      expect(ctl.guardDeviceInput(msg), `媒体模式应拒绝 ${msg.type}`).toBe(false)
    }
    // 非设备输入（如本地音频开关）不拦
    expect(ctl.guardDeviceInput({ type: 'audio', on: true })).toBe(true)

    ctl.view.backToLive()
    await flushPromises()
    for (const msg of inputs) expect(ctl.guardDeviceInput(msg)).toBe(true)
    wrapper.unmount()
  })

  it('surfaceEl 随来源切换：live = 实时视频元素，media = 媒体 video 元素', async () => {
    const liveEl = { videoWidth: 1920, videoHeight: 1080 }
    const { ctl, wrapper } = mountStage({ liveVideo: liveEl })
    expect(ctl.surfaceEl()).toBe(liveEl)
    ctl.view.toggleKind()
    await flushPromises()
    expect(ctl.surfaceEl()).toBeNull() // 媒体元素未挂载
    const mediaEl = fakeMediaVideoEl()
    ctl.attachMediaVideo(mediaEl)
    expect(ctl.surfaceEl()).toBe(mediaEl)
    ctl.detachMediaVideo()
    wrapper.unmount()
  })
})

describe('useConsoleStage：媒体播放控制与指定帧', () => {
  async function mountInMedia(extra = {}) {
    api.listMedia.mockResolvedValue([{ id: 'm1', name: 'a.mp4', duration_us: 10000000, width: 1280, height: 720 }])
    const { ctl, wrapper, connected } = mountStage(extra)
    ctl.view.toggleKind()
    await flushPromises()
    const el = fakeMediaVideoEl()
    ctl.attachMediaVideo(el)
    await flushPromises()
    return { ctl, wrapper, el, connected }
  }

  it('frameAt/displaySize 随媒体画面：pts = 预览时间 × 1e6（微秒取整）', async () => {
    const { ctl, wrapper, el } = await mountInMedia()
    expect(ctl.view.stageReady).toBe(true)
    expect(ctl.displaySize()).toEqual({ width: 1280, height: 720 })
    el.currentTime = 1.5
    el.emit('timeupdate')
    expect(ctl.frameAt()).toEqual({ mediaId: 'm1', ptsUs: 1500000, index: null })
    expect(ctl.view.timeText).toBe('00:01.5')
    expect(ctl.view.durationText).toBe('00:10.0')
    wrapper.unmount()
  })

  it('逐帧步进：服务端真实展示帧表相邻定位（首步解析 → 锁定后沿相邻链走）', async () => {
    const { ctl, wrapper, el } = await mountInMedia()
    el.currentTime = 0.5
    el.emit('timeupdate')
    // 首步：mediaFrames(pts_us) 解析当前帧 + neighbors(index) 取相邻
    fetch.mockResolvedValueOnce(jsonRes(200, {
      frame_count: 30, first_pts_us: 0, last_pts_us: 966666,
      current: { index: 15, pts_us: 500000 },
    }))
    fetch.mockResolvedValueOnce(jsonRes(200, {
      index: 15, pts_us: 500000,
      prev: { index: 14, pts_us: 466666 }, next: { index: 16, pts_us: 533333 },
    }))
    el.paused = false
    await ctl.view.stepFrames(-1)
    expect(el.paused).toBe(true, '逐帧步进自动暂停')
    expect(el.currentTime).toBeCloseTo(0.466666, 9)
    expect(fetch.mock.calls[0][0]).toBe('/api/media/m1/frames?pts_us=500000')
    expect(fetch.mock.calls[1][0]).toBe('/api/media/m1/frames/15')
    // 锁定帧身份后 frameAt 携带展示序索引
    expect(ctl.frameAt()).toEqual({ mediaId: 'm1', ptsUs: 466666, index: 14 })

    // 第二步不再解析（帧身份已锁定），直接沿相邻链走
    fetch.mockResolvedValueOnce(jsonRes(200, {
      index: 14, pts_us: 466666,
      prev: { index: 13, pts_us: 433333 }, next: { index: 15, pts_us: 500000 },
    }))
    await ctl.view.stepFrames(-1)
    expect(el.currentTime).toBeCloseTo(0.433333, 9)
    expect(fetch.mock.calls[2][0]).toBe('/api/media/m1/frames/14')

    // 浏览器 seek 完成（程序性 seek 消费标记，帧身份保持）；用户手动 seek → 身份失效
    el.emit('seeked')
    expect(ctl.frameAt().index).toBe(13)
    el.currentTime = 5
    el.emit('seeked')
    expect(ctl.frameAt().index).toBeNull()
    expect(ctl.frameAt().ptsUs).toBe(5000000)

    // 末帧边界：next = null 不动（无帧可取、不发精确帧请求）
    fetch.mockResolvedValueOnce(jsonRes(200, {
      frame_count: 30, first_pts_us: 0, last_pts_us: 966666,
      current: { index: 29, pts_us: 966666 },
    }))
    fetch.mockResolvedValueOnce(jsonRes(200, {
      index: 29, pts_us: 966666, prev: { index: 28, pts_us: 933333 }, next: null,
    }))
    const callsBefore = fetch.mock.calls.length
    await ctl.view.stepFrames(1)
    expect(el.currentTime).toBe(5)
    expect(fetch.mock.calls.length).toBe(callsBefore + 2)
    wrapper.unmount()
  })

  it('帧表不可用：stepFrames 提示且不改预览位置（不做时间近似降级）', async () => {
    const { ctl, wrapper, el } = await mountInMedia()
    el.currentTime = 1
    el.emit('timeupdate')
    fetch.mockResolvedValueOnce(jsonRes(500, { error: 'internal' }))
    await ctl.view.stepFrames(1)
    expect(el.currentTime).toBe(1)
    wrapper.unmount()
  })

  it('captureFrame：live = 现有实时视频元素；media = 服务端确定帧 PNG（未锁定按 ptsUs，锁定后按展示序索引）', async () => {
    const liveEl = { videoWidth: 1920, videoHeight: 1080 }
    const { ctl, wrapper } = mountStage({ liveVideo: liveEl })
    const liveFrame = await ctl.captureFrame()
    expect(liveFrame).toMatchObject({ width: 1920, height: 1080, generation: 0, label: '实时画面当前帧' })
    expect(liveFrame.source).toBe(liveEl)
    wrapper.unmount()

    const media = await mountInMedia({
      loadImage: async (url) => {
        expect(url).toContain('/api/media/m1/frame')
        expect(url).toContain('pts_us=')
        return { naturalWidth: 640, naturalHeight: 360 }
      },
    })
    const { ctl: ctl2, wrapper: w2, el: el2 } = media
    el2.currentTime = 2
    el2.emit('timeupdate')
    const frame = await ctl2.captureFrame()
    expect(frame).toMatchObject({ width: 640, height: 360, label: '视频帧 @ 00:02.0' })
    expect(typeof frame.generation).toBe('number')
    w2.unmount()

    // 帧身份锁定后（逐帧步进过）：captureFrame 按展示序索引寻址，label 携带帧号
    const media3 = await mountInMedia({
      loadImage: async (url) => {
        expect(url).toBe('/api/media/m1/frame?index=14')
        return { naturalWidth: 640, naturalHeight: 360 }
      },
    })
    const { ctl: ctl3, wrapper: w3, el: el3 } = media3
    el3.currentTime = 0.5
    el3.emit('timeupdate')
    fetch.mockResolvedValueOnce(jsonRes(200, {
      frame_count: 30, first_pts_us: 0, last_pts_us: 966666,
      current: { index: 15, pts_us: 500000 },
    }))
    fetch.mockResolvedValueOnce(jsonRes(200, {
      index: 15, pts_us: 500000,
      prev: { index: 14, pts_us: 466666 }, next: { index: 16, pts_us: 533333 },
    }))
    await ctl3.view.stepFrames(-1)
    const frame3 = await ctl3.captureFrame()
    expect(frame3).toMatchObject({ width: 640, height: 360, label: '视频帧 #14 @ 00:00.5' })
    w3.unmount()
  })

  it('录制按钮态：activeRecording 轮询驱动；开始需实时已连接；停止后打开新素材', async () => {
    api.activeRecording.mockResolvedValue(null)
    const { ctl, wrapper, connected } = await mountInMedia()
    // 媒体模式不产生新录制（start 需实时来源 + 已连接）
    await ctl.view.toggleRecording()
    expect(api.recordingStart).not.toHaveBeenCalled()

    ctl.view.backToLive()
    await flushPromises()
    connected.value = false
    await ctl.view.toggleRecording()
    expect(api.recordingStart).not.toHaveBeenCalled()

    connected.value = true
    api.recordingStart.mockResolvedValueOnce({ id: 'r-9', state: 'recording' })
    await ctl.view.toggleRecording()
    expect(api.recordingStart).toHaveBeenCalledWith('dev-a')
    expect(ctl.view.recordingActive).toBe(true)

    // 停止：服务端终态 + 产出素材自动打开（切到视频来源）
    api.recordingStop.mockResolvedValueOnce({
      id: 'r-9', state: 'completed',
      segments: [{ media_id: 'm-new', start_us: 0, duration_us: 1000000, reason: 'normal' }],
    })
    api.listMedia.mockResolvedValue([{ id: 'm-new', name: 'rec.mp4' }])
    api.getMedia.mockResolvedValue({ id: 'm-new', name: 'rec.mp4', duration_us: 1000000 })
    await ctl.view.toggleRecording()
    expect(api.recordingStop).toHaveBeenCalledWith('r-9')
    expect(ctl.view.recordingActive).toBe(false)
    expect(ctl.view.kind).toBe('media')
    expect(ctl.view.mediaId).toBe('m-new')
    wrapper.unmount()
  })

  it('formatStageClock：秒 → mm:ss.d（非法值归 0）', () => {
    expect(formatStageClock(0)).toBe('00:00.0')
    expect(formatStageClock(65.43)).toBe('01:05.4')
    expect(formatStageClock(1500000 / 1e6)).toBe('00:01.5')
    expect(formatStageClock(-1)).toBe('00:00.0')
    expect(formatStageClock(Number.NaN)).toBe('00:00.0')
  })
})

// ---------- 指定帧裁切（TemplateCropModal 数据流） ----------
function mountTemplates({ stage, connectedValue = true, videoEl = null } = {}) {
  let tpl
  const videoElement = ref(videoEl)
  const videoWrap = ref({ getBoundingClientRect: () => ({ left: 0, top: 0, width: 960, height: 540 }) })
  const Host = defineComponent({
    setup() {
      tpl = useConsoleTemplates({
        toast: vi.fn(),
        store: reactive({ deviceId: 'dev-a' }),
        templatesData: ref([]),
        packageId: ref('pkg-a'),
        connected: ref(connectedValue),
        videoElement,
        videoWrap,
        current: ref({ width: 1920, height: 1080 }),
        stage,
      })
      return () => h('div')
    },
  })
  const wrapper = mount(Host)
  return { tpl, wrapper }
}

function mediaStageBridge(genRef, captureImpl) {
  const mediaEl = {
    videoWidth: 640,
    videoHeight: 360,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 960, height: 540 }),
  }
  return {
    kind: () => 'media',
    ready: () => true,
    generation: () => genRef.value,
    surfaceEl: () => mediaEl,
    captureFrame: captureImpl,
  }
}

describe('useConsoleTemplates：指定帧裁切（合同 §4/§6）', () => {
  beforeEach(() => stubCanvasCreation())

  it('媒体来源裁切：底图 = captureFrame 指定帧图像，坐标系用素材帧尺寸（displaySize）', async () => {
    const genRef = { value: 3 }
    const frameImage = { naturalWidth: 640, naturalHeight: 360 }
    let captureCalls = 0
    const stage = mediaStageBridge(genRef, async () => {
      captureCalls += 1
      return { source: frameImage, width: 640, height: 360, generation: 3, label: '视频帧 @ 00:01.5' }
    })
    const { tpl } = mountTemplates({ stage })
    await tpl.openCrop({ x: 10, y: 10, w: 100, h: 50 })
    await flushPromises()
    expect(captureCalls).toBe(1)
    expect(tpl.crop.active).toBe(true)
    // 坐标系 = 指定帧原始尺寸（而非实时画面的 1920×1080）
    expect(tpl.crop.imgW).toBe(640)
    expect(tpl.crop.imgH).toBe(360)
    expect(tpl.crop.sourceLabel).toContain('视频帧')
    // 默认模板名区域后缀按素材帧尺寸归一（10/640→016，10/360→028，110/640→172，60/360→167）
    expect(tpl.crop.name).toContain('#016_028_172_167')
    // 坐标换算随舞台来源（surface = 媒体画面 640×360）
    expect(tpl.toDeviceCoord(480, 270)).toEqual({ x: 320, y: 180 })
  })

  it('安全红线：取帧期间来源切换（generation 变化）→ 过期结果不应用，不进入裁切', async () => {
    const genRef = { value: 7 }
    let resolveCapture
    const stage = mediaStageBridge(genRef, () => new Promise(resolve => { resolveCapture = resolve }))
    const { tpl } = mountTemplates({ stage })
    const pending = tpl.openCrop({ x: 0, y: 0, w: 50, h: 50 })
    genRef.value += 1 // 取帧期间用户切换了来源/素材
    resolveCapture({ source: {}, width: 640, height: 360, generation: 7, label: '过期帧' })
    await pending
    await flushPromises()
    expect(tpl.crop.active).toBe(false)
  })

  it('实时来源裁切保持既有路径：同步冻结实时视频元素当前画面', () => {
    const liveEl = {
      videoWidth: 1920,
      videoHeight: 1080,
      getBoundingClientRect: () => ({ left: 0, top: 0, width: 960, height: 540 }),
    }
    let captureCalls = 0
    const stage = {
      kind: () => 'live',
      ready: () => true,
      generation: () => 1,
      surfaceEl: () => liveEl,
      captureFrame: async () => { captureCalls += 1; return null },
    }
    const { tpl } = mountTemplates({ stage })
    tpl.openCrop({ x: 10, y: 10, w: 100, h: 50 })
    // live 路径同步完成，不调 captureFrame（现有截图路径不变）
    expect(captureCalls).toBe(0)
    expect(tpl.crop.active).toBe(true)
    expect(tpl.crop.imgW).toBe(1920)
    expect(tpl.crop.imgH).toBe(1080)
    expect(tpl.crop.sourceLabel).toBe('实时画面当前帧')
  })

  it('togglePick 可用性随舞台来源；框选矩形随 surface 换算', () => {
    const genRef = { value: 0 }
    const stage = mediaStageBridge(genRef, async () => null)
    const { tpl } = mountTemplates({ stage, connectedValue: false })
    // 未连接设备但媒体画面就绪 → 可框选（离线框选）
    tpl.togglePick()
    expect(tpl.picking.value).toBe(true)
    // 媒体不可用（ready=false）→ 拒绝并提示
    const offline = { ...stage, ready: () => false }
    const t2 = mountTemplates({ stage: offline, connectedValue: false })
    t2.tpl.togglePick()
    expect(t2.tpl.picking.value).toBe(false)
  })
})
