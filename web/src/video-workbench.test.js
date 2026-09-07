// @vitest-environment happy-dom
/**
 * 视频工作台面板（gamer.video core 面板）挂载测试：
 * - 宿主 VideoWorkbench 三区装配 + 素材列表 → 时间轴选中预览（mediaFileUrl）；
 * - 素材库删除两段确认（armed → 确认删除），409 = 被项目引用提示；
 * - 录制入口：activeRecording 轮询驱动开始/停止按钮态，停止后上抛 recording-finished；
 * - 时间轴：精确帧（currentTime*1e6 → pts_us 的服务端 PNG URL）+ 逐帧 ±33ms 步进
 *   （seeked 后自动取帧）+ 帧加载失败提示 + 切素材清空；
 * - 草稿区状态流转：载入事件（时间轴升序）→ 勾选 → 生成 YAML 草稿（yaml + 诊断），
 *   常驻「草稿不会自动执行」标注。
 * videoApi 模块整体 mock（端点形态锁定在 video-api.test.js）。
 */
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('./components/video/videoApi', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    videoApi: {
      ...actual.videoApi,
      listMedia: vi.fn(async () => []),
      importMedia: vi.fn(async () => ({})),
      deleteMedia: vi.fn(async () => null),
      recordingStart: vi.fn(async () => ({})),
      recordingStop: vi.fn(async () => ({})),
      recordingCancel: vi.fn(async () => ({})),
      activeRecording: vi.fn(async () => null),
      recordingEvents: vi.fn(async () => []),
      createVideoDraft: vi.fn(async () => ({ yaml: '', diagnostics: [] })),
    },
  }
})

import MediaLibrary from './components/video/MediaLibrary.vue'
import VideoDraft from './components/video/VideoDraft.vue'
import VideoTimeline from './components/video/VideoTimeline.vue'
import VideoWorkbench from './components/video/VideoWorkbench.vue'
import { videoApi } from './components/video/videoApi'
import { devicesData } from './store'

const MEDIA = [
  { id: 'm1', name: '录制-0907.mp4', duration_us: 65000000, width: 1920, height: 1080, source: 'recording', state: 'ready' },
  { id: 'm2', name: '导入素材.mp4', duration_us: 3200000, width: 1280, height: 720, source: 'import', state: 'ready' },
]

const SESSION = { id: 'rec-1', device_id: 'dev-a', state: 'recording', event_count: 7 }
const FINISHED = { id: 'rec-1', device_id: 'dev-a', state: 'completed', event_count: 9 }

const EVENTS = [
  { event_id: 'e2', kind: 'tap', source: 'manual', timeline_us: 5200000, payload: { x: 100, y: 200 } },
  { event_id: 'e1', kind: 'swipe', source: 'manual', timeline_us: 1200000, payload: { x: 1, y: 2, x2: 3, y2: 4 } },
]

beforeEach(() => {
  vi.clearAllMocks()
  devicesData.value = [{ id: 'dev-a', name: '设备A' }, { id: 'dev-b', name: '设备B' }]
  videoApi.listMedia.mockResolvedValue(MEDIA.map(m => ({ ...m })))
  videoApi.activeRecording.mockResolvedValue(null)
})

describe('VideoWorkbench 宿主装配', () => {
  it('挂载即拉取素材列表，三区齐全；选中素材 → 时间轴出现 mediaFileUrl 预览', async () => {
    const w = mount(VideoWorkbench)
    await flushPromises()

    expect(videoApi.listMedia).toHaveBeenCalled()
    expect(w.find('[data-testid="media-library"]').exists()).toBe(true)
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(true)
    expect(w.find('[data-testid="video-draft"]').exists()).toBe(true)
    expect(w.findAll('[data-testid="media-row"]')).toHaveLength(2)
    // 未选中素材时时间轴为空态，不渲染 video
    expect(w.find('video').exists()).toBe(false)

    await w.findAll('[data-testid="media-row"]')[0].trigger('click')
    const video = w.find('video')
    expect(video.exists()).toBe(true)
    // 合同 §1：播放流 URL = /api/media/:id/file
    expect(video.attributes('src')).toBe('/api/media/m1/file')
    expect(w.find('[data-testid="video-time"]').text()).toBe('0.000s')
    w.unmount()
  })

  it('选中素材被删除后回落空态（不自动跳选）', async () => {
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.findAll('[data-testid="media-row"]')[0].trigger('click')
    expect(w.find('video').exists()).toBe(true)

    videoApi.listMedia.mockResolvedValue(MEDIA.slice(1))
    await w.find('[data-testid="media-refresh"]').trigger('click')
    await flushPromises()
    expect(w.find('video').exists()).toBe(false)
    w.unmount()
  })
})

describe('MediaLibrary 素材库区', () => {
  it('导入：选择文件 → importMedia(字节, 文件名) → changed 事件触发刷新', async () => {
    const w = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    const bytes = new Uint8Array([9, 9])
    const file = new File([bytes], '新素材.mp4', { type: 'video/mp4' })
    const input = w.find('[data-testid="media-file-input"]')
    Object.defineProperty(input.element, 'files', { value: [file], configurable: true })
    await input.trigger('change')
    await flushPromises()

    expect(videoApi.importMedia).toHaveBeenCalledTimes(1)
    const [argBytes, argName] = videoApi.importMedia.mock.calls[0]
    expect(argName).toBe('新素材.mp4')
    expect(Array.from(argBytes)).toEqual([9, 9])
    expect(w.emitted('changed')).toHaveLength(1)
    w.unmount()
  })

  it('删除两段确认：armed → 确认删除 → deleteMedia + changed；409 提示被项目引用', async () => {
    const w = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    const deleteBtn = () => w.findAll('[data-testid="media-row"]')[0].find('button')

    await deleteBtn().trigger('click')
    expect(deleteBtn().text()).toBe('确认删除')
    expect(videoApi.deleteMedia).not.toHaveBeenCalled()

    await deleteBtn().trigger('click')
    expect(videoApi.deleteMedia).toHaveBeenCalledWith('m1')
    expect(w.emitted('changed')).toHaveLength(1)
    w.unmount()

    const w2 = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    videoApi.deleteMedia.mockRejectedValueOnce(Object.assign(new Error('x'), { status: 409 }))
    const btn2 = () => w2.findAll('[data-testid="media-row"]')[0].find('button')
    await btn2().trigger('click')
    await btn2().trigger('click')
    await flushPromises()
    expect(w2.find('[data-testid="media-error"]').text()).toContain('被项目引用')
    w2.unmount()
  })

  it('录制入口：选中设备后轮询活动会话驱动按钮态；停止 → recording-finished + changed', async () => {
    const w = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    await flushPromises()
    // 未选设备：只有开始按钮（禁用），不轮询
    expect(w.find('[data-testid="record-start"]').exists()).toBe(true)
    expect(w.find('[data-testid="record-start"]').attributes('disabled')).toBeDefined()
    expect(videoApi.activeRecording).not.toHaveBeenCalled()

    await w.find('[data-testid="record-device"]').setValue('dev-a')
    await flushPromises()
    expect(videoApi.activeRecording).toHaveBeenCalledWith('dev-a')
    // 无活动会话 → 开始按钮可用
    expect(w.find('[data-testid="record-start"]').attributes('disabled')).toBeUndefined()

    videoApi.activeRecording.mockResolvedValue(SESSION)
    videoApi.recordingStop.mockResolvedValue(FINISHED)
    // 触发一次重新轮询（切走再切回）
    await w.find('[data-testid="record-device"]').setValue('dev-b')
    await w.find('[data-testid="record-device"]').setValue('dev-a')
    await flushPromises()
    // 活动会话出现 → 开始消失，停止/取消出现 + 状态行
    expect(w.find('[data-testid="record-start"]').exists()).toBe(false)
    expect(w.find('[data-testid="record-stop"]').exists()).toBe(true)
    expect(w.find('[data-testid="record-state"]').text()).toContain('录制中')

    await w.find('[data-testid="record-stop"]').trigger('click')
    await flushPromises()
    expect(videoApi.recordingStop).toHaveBeenCalledWith('rec-1')
    expect(w.emitted('recording-finished')).toEqual([[FINISHED]])
    expect(w.emitted('changed')).toHaveLength(1)
    w.unmount()
  })
})

describe('VideoTimeline 时间轴区（纯离线，不触达设备）', () => {
  it('精确帧：以 currentTime*1e6 为 pts_us 展示服务端 PNG；帧加载失败给出错误提示', async () => {
    const w = mount(VideoTimeline, { props: { media: MEDIA[0] } })
    const video = w.find('video')
    // 合同 §1：播放流 = /api/media/:id/file
    expect(video.attributes('src')).toBe('/api/media/m1/file')
    expect(w.find('[data-testid="frame-box"]').exists()).toBe(false)

    video.element.currentTime = 1.5
    await video.trigger('timeupdate')
    expect(w.find('[data-testid="video-time"]').text()).toBe('1.500s')

    await w.find('[data-testid="frame-exact"]').trigger('click')
    const img = w.find('[data-testid="frame-image"]')
    expect(img.exists()).toBe(true)
    expect(img.attributes('src')).toBe('/api/media/m1/frame?pts_us=1500000&max_width=640')

    await img.trigger('error')
    expect(w.find('[data-testid="frame-error"]').exists()).toBe(true)
    expect(w.find('[data-testid="frame-exact"]').attributes('disabled')).toBeUndefined()
    w.unmount()
  })

  it('逐帧 +：预览步进 33ms，seeked 后按新时间自动取精确帧', async () => {
    const w = mount(VideoTimeline, { props: { media: MEDIA[0] } })
    const video = w.find('video')
    video.element.currentTime = 2
    await video.trigger('timeupdate')

    await w.find('[data-testid="frame-next"]').trigger('click')
    expect(video.element.currentTime).toBeCloseTo(2.033, 6)
    // happy-dom 不自动派发 seeked；对应真实浏览器 seek 完成后自动取帧
    await video.trigger('seeked')
    const img = w.find('[data-testid="frame-image"]')
    expect(img.exists()).toBe(true)
    expect(img.attributes('src')).toBe('/api/media/m1/frame?pts_us=2033000&max_width=640')
    w.unmount()
  })

  it('切换素材后帧区清空回到空态', async () => {
    const w = mount(VideoTimeline, { props: { media: MEDIA[0] } })
    await w.find('[data-testid="frame-exact"]').trigger('click')
    expect(w.find('[data-testid="frame-box"]').exists()).toBe(true)

    await w.setProps({ media: MEDIA[1] })
    expect(w.find('[data-testid="frame-box"]').exists()).toBe(false)
    expect(w.find('video').attributes('src')).toBe('/api/media/m2/file')
    w.unmount()
  })
})

describe('VideoDraft 草稿区状态流转', () => {
  const mountDraft = (recordingId = 'rec-9') => mount(VideoDraft, { props: { recordingId } })

  it('载入事件：时间轴升序渲染 kind/时间/来源；全选/清空驱动已选数', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()

    expect(videoApi.recordingEvents).toHaveBeenCalledWith('rec-9')
    const rows = wrapper.findAll('.event-row')
    expect(rows).toHaveLength(2)
    // 服务端升序合同 + 客户端防御性排序：e1(1.2s) 在 e2(5.2s) 前
    expect(rows[0].text()).toContain('swipe')
    expect(rows[0].text()).toContain('1.200s')
    expect(rows[1].text()).toContain('tap')
    expect(wrapper.find('[data-testid="draft-generate"]').attributes('disabled')).toBeDefined()

    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    expect(wrapper.find('[data-testid="draft-generate"]').text()).toContain('草稿（2）')
    await wrapper.find('[data-testid="draft-clear"]').trigger('click')
    expect(wrapper.find('[data-testid="draft-generate"]').attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('勾选 → 生成 YAML 草稿：createVideoDraft(recordingId, 选中 ids)，展示 yaml 与诊断，常驻不执行标注', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    videoApi.createVideoDraft.mockResolvedValue({
      yaml: 'version: 3\nsteps:\n  - tap: [100, 200]',
      diagnostics: [{ event_id: 'e2', reason: 'multi_touch_not_supported' }],
    })
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()

    // 只勾第一条（swipe）
    await wrapper.findAll('.event-check')[0].setValue(true)
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()

    expect(videoApi.createVideoDraft).toHaveBeenCalledWith('rec-9', ['e1'])
    expect(wrapper.find('[data-testid="draft-yaml"]').text()).toContain('tap: [100, 200]')
    const diags = wrapper.find('[data-testid="draft-diagnostics"]')
    expect(diags.text()).toContain('e2')
    expect(diags.text()).toContain('multi_touch_not_supported')
    // 合同要求：明确标注草稿不会自动执行
    expect(wrapper.text()).toContain('草稿不会自动执行')
    wrapper.unmount()
  })

  it('生成失败 / 空事件会话走错误与空态分支', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue([])
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('没有可映射的操作事件')
    wrapper.unmount()

    const wrapper2 = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    videoApi.createVideoDraft.mockRejectedValue(new Error('extension not running'))
    await wrapper2.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()
    await wrapper2.find('[data-testid="draft-select-all"]').trigger('click')
    await wrapper2.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    expect(wrapper2.find('[data-testid="draft-error"]').text()).toContain('extension not running')
    wrapper2.unmount()
  })

  it('录制完成后宿主带入会话 id：recordingId prop 变化自动载入事件', async () => {
    const wrapper = mount(VideoDraft, { props: { recordingId: '' } })
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    await wrapper.setProps({ recordingId: 'rec-77' })
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-recording-id"]').element.value).toBe('rec-77')
    expect(videoApi.recordingEvents).toHaveBeenCalledWith('rec-77')
    wrapper.unmount()
  })

  it('mountDraft 帮助函数路径：未载入事件前生成按钮不出现', async () => {
    const wrapper = mountDraft()
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-generate"]').exists()).toBe(false)
    wrapper.unmount()
  })
})
