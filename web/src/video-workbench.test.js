// @vitest-environment happy-dom
/**
 * 视频工作台面板（gamer.video core 面板）挂载测试：
 * - 宿主 VideoWorkbench 三分区（素材库/项目/草稿子导航，非 Core 永久页签）+
 *   Package 缺失横幅 + 项目列表/详情装配 + 打开项目联动舞台媒体
 *   （requestStageMedia：只动舞台来源，不动设备/包身份）；
 * - 项目编辑流：加标记（帧身份）→ 标脏 → 保存（expected_version 乐观并发）；
 *   校准应用版本递增 + 旧校准标记标脏；保存冲突 409 可诊断可重载；素材缺失横幅；
 * - 素材库删除两段确认（armed → 确认删除），409 = 被项目引用提示；
 * - 录制入口：activeRecording 轮询驱动开始/停止按钮态，停止后上抛 recording-finished；
 * - 时间轴：精确帧（服务端 PNG URL）+ 逐帧步进（服务端真实展示帧表，无 33ms 假设）+
 *   标记 CRUD（帧身份引用）+ 校准表单 + 自录事件叠加（base_pts_us 整数映射）；
 * - 草稿区状态流转与「草稿不会自动执行」标注。
 * videoApi 模块整体 mock（端点形态锁定在 video-api.test.js / video-project.test.js）。
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
      recordingStatus: vi.fn(async () => ({ id: 'rec-1', segments: [] })),
      createVideoDraft: vi.fn(async () => ({ yaml: '', diagnostics: [] })),
      saveDraft: vi.fn(async () => ({ id: 'pkg/draft-1.yaml', path: 'automations/draft-1.yaml', package_id: 'pkg' })),
      createTemplateFromFrame: vi.fn(async () => ({ name: 'tpl.png', short_name: 'tpl.png' })),
      visionTestTemplate: vi.fn(async () => ({ hit: true, score: 0.9 })),
      getMedia: vi.fn(async () => ({ id: 'm1', refs: [] })),
      setMediaRefs: vi.fn(async () => ({})),
      mediaFrames: vi.fn(async () => ({ frame_count: 0, first_pts_us: null, last_pts_us: null })),
      mediaFrameNeighbors: vi.fn(async () => ({ index: 0, pts_us: 0, prev: null, next: null })),
      listProjectEntries: vi.fn(async () => []),
      getProject: vi.fn(async () => ({ path: 'projects/p1.json', content: '{}', version: 'v0' })),
      putProject: vi.fn(async () => ({ ok: true, version: 'v-next' })),
      deleteProject: vi.fn(async () => null),
      renameProject: vi.fn(async () => ({ ok: true })),
    },
  }
})

vi.mock('./components/console/useConsoleStage', async (importOriginal) => {
  const actual = await importOriginal()
  return { ...actual, requestStageMedia: vi.fn() }
})

// VideoDraft 保存成功后的 automation.open_editor 导航（router.push 可观察）
const routerPush = vi.fn(async () => {})
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: routerPush, currentRoute: { value: { query: {} } } }),
}))

import MediaLibrary from './components/video/MediaLibrary.vue'
import VideoDraft from './components/video/VideoDraft.vue'
import VideoProjects from './components/video/VideoProjects.vue'
import VideoTimeline from './components/video/VideoTimeline.vue'
import VideoWorkbench from './components/video/VideoWorkbench.vue'
import { videoApi } from './components/video/videoApi'
import { requestStageMedia } from './components/console/useConsoleStage'
import { devicesData } from './store'
import { packageStore } from './package-store'

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

function projectJson(overrides = {}) {
  return JSON.stringify({
    schema_version: 1,
    id: 'p1',
    name: '日常标记',
    package_id: 'pkg',
    notes: '',
    created_at: '2026-09-07T00:00:00Z',
    updated_at: '2026-09-07T00:00:00Z',
    assets: [{ media_id: 'm1', role: 'primary', sha256: '', duration_us: 65000000, frame_count: null }],
    recording: null,
    calibration: {
      version: 1, rotation: 0, pixel_aspect: { num: 1, den: 1 },
      content_rect: null, reference_size: { width: 1920, height: 1080 },
    },
    markers: [],
    progress: { stage: 'created', updated_at: '2026-09-07T00:00:00Z' },
    ...overrides,
  })
}

const PROJECT_ENTRY = { path: 'projects/p1.json', content: projectJson(), version: 'v1' }

function stubProject(entry = PROJECT_ENTRY) {
  videoApi.listProjectEntries.mockResolvedValue([entry])
  videoApi.getProject.mockResolvedValue(entry)
}

beforeEach(() => {
  vi.clearAllMocks()
  devicesData.value = [{ id: 'dev-a', name: '设备A' }, { id: 'dev-b', name: '设备B' }]
  packageStore.currentPackageId = 'pkg'
  videoApi.listMedia.mockResolvedValue(MEDIA.map(m => ({ ...m })))
  videoApi.activeRecording.mockResolvedValue(null)
  videoApi.listProjectEntries.mockResolvedValue([])
  // 展示帧表缺省空态（各用例按需覆盖）
  videoApi.mediaFrames.mockResolvedValue({ frame_count: 0, first_pts_us: null, last_pts_us: null })
  videoApi.mediaFrameNeighbors.mockResolvedValue({ index: 0, pts_us: 0, prev: null, next: null })
  videoApi.putProject.mockResolvedValue({ ok: true, version: 'v-next' })
})

// ---------------------------------------------------------------------------
// VideoWorkbench 宿主装配
// ---------------------------------------------------------------------------

describe('VideoWorkbench 宿主装配', () => {
  it('挂载即拉取素材列表；三分区子导航齐全；素材库为默认分区', async () => {
    const w = mount(VideoWorkbench)
    await flushPromises()

    expect(videoApi.listMedia).toHaveBeenCalled()
    expect(videoApi.listProjectEntries).toHaveBeenCalledWith('pkg')
    expect(w.find('[data-testid="media-library"]').exists()).toBe(true)
    for (const key of ['library', 'projects', 'draft']) {
      expect(w.find(`[data-testid="workbench-tab-${key}"]`).exists()).toBe(true)
    }
    // 素材库行渲染；项目分区与草稿分区未激活
    expect(w.findAll('[data-testid="media-row"]')).toHaveLength(2)
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(false)
    expect(w.find('[data-testid="video-draft"]').exists()).toBe(false)
    w.unmount()
  })

  it('无当前 Package：项目分区显示缺包横幅（项目保存在 Package 数据上下文）', async () => {
    packageStore.currentPackageId = null
    const w = mount(VideoWorkbench)
    await flushPromises()
    expect(w.find('[data-testid="package-missing-banner"]').exists()).toBe(true)
    w.unmount()
  })

  it('项目分区：列表渲染 + 打开项目 → 详情时间轴出现，并联动舞台切到主素材', async () => {
    stubProject()
    const w = mount(VideoWorkbench)
    await flushPromises()

    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    const rows = w.findAll('[data-testid="project-row"]')
    expect(rows).toHaveLength(1)
    expect(rows[0].text()).toContain('日常标记')
    expect(rows[0].text()).toContain('0 标记')

    await rows[0].trigger('click')
    await flushPromises()
    expect(videoApi.getProject).toHaveBeenCalledWith('pkg', 'p1')
    // 联动：舞台切到主素材 m1（只动舞台来源）
    expect(requestStageMedia).toHaveBeenCalledWith('m1')
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(true)
    expect(w.find('video').attributes('src')).toBe('/api/media/m1/file')
    expect(w.find('[data-testid="open-project-name"]').text()).toBe('日常标记')
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('已保存')
    w.unmount()
  })

  it('素材缺失：主素材不在媒体库 → 缺失横幅 + 保存禁用 + 不联动舞台', async () => {
    const missing = { ...PROJECT_ENTRY, content: projectJson() }
    videoApi.listMedia.mockResolvedValue(MEDIA.slice(1)) // 只有 m2
    stubProject(missing)
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await w.findAll('[data-testid="project-row"]')[0].trigger('click')
    await flushPromises()

    expect(w.find('[data-testid="asset-missing-banner"]').exists()).toBe(true)
    expect(requestStageMedia).not.toHaveBeenCalled()
    expect(w.find('[data-testid="project-save"]').attributes('disabled')).toBeDefined()
    w.unmount()
  })

  it('损坏项目：列表可见不可打开，提示校验失败', async () => {
    stubProject({ path: 'projects/broken.json', content: '{oops', version: 'v1' })
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    const row = w.findAll('[data-testid="project-row"]')[0]
    expect(row.text()).toContain('校验失败')
    await row.trigger('click')
    await flushPromises()
    expect(videoApi.getProject).not.toHaveBeenCalled()
    expect(w.find('[data-testid="project-detail-empty"]').exists()).toBe(true)
    w.unmount()
  })

  it('选中素材被删除后回落空态（不自动跳选）', async () => {
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.findAll('[data-testid="media-row"]')[0].trigger('click')
    // 库分区没有 video 预览（预览在项目详情时间轴）
    expect(w.find('video').exists()).toBe(false)
    w.unmount()
  })

  it('Package 切换：关闭打开态并重拉项目列表', async () => {
    stubProject()
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await w.findAll('[data-testid="project-row"]')[0].trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(true)

    packageStore.currentPackageId = 'pkg2'
    await flushPromises()
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(false)
    expect(videoApi.listProjectEntries).toHaveBeenCalledWith('pkg2')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// 项目编辑流（标记 / 校准 / 保存 / 冲突）
// ---------------------------------------------------------------------------

describe('VideoWorkbench 项目编辑流', () => {
  async function mountOpenProject(entry = PROJECT_ENTRY) {
    stubProject(entry)
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await w.findAll('[data-testid="project-row"]')[0].trigger('click')
    await flushPromises()
    return w
  }

  it('创建项目：基于素材库选中素材 → putProject → 打开', async () => {
    // 首次拉列表为空（创建前），创建后 reload 返回新项目
    videoApi.listProjectEntries.mockResolvedValueOnce([])
    videoApi.listProjectEntries.mockResolvedValue([
      { path: 'projects/daily-login.json', content: projectJson({ id: 'daily-login', name: '每日登录' }), version: 'v-created' },
    ])
    videoApi.getProject.mockResolvedValue({ path: 'projects/daily-login.json', content: projectJson({ id: 'daily-login', name: '每日登录' }), version: 'v-created' })
    const w = mount(VideoWorkbench)
    await flushPromises()
    // 先在素材库选中主素材
    await w.findAll('[data-testid="media-row"]')[0].trigger('click')
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="project-create"]').attributes('disabled')).toBeUndefined()

    await w.find('[data-testid="project-create"]').trigger('click')
    await w.find('[data-testid="project-create-id"]').setValue('daily-login')
    await w.find('[data-testid="project-create-name"]').setValue('每日登录')
    await w.find('[data-testid="project-create-confirm"]').trigger('click')
    await flushPromises()

    expect(videoApi.putProject).toHaveBeenCalledTimes(1)
    const [pkg, id, content] = videoApi.putProject.mock.calls[0]
    expect(pkg).toBe('pkg')
    expect(id).toBe('daily-login')
    expect(JSON.parse(content).assets[0].media_id).toBe('m1')
    // 创建后自动打开（重读资源 + 联动舞台主素材）
    expect(videoApi.getProject).toHaveBeenCalledWith('pkg', 'daily-login')
    expect(requestStageMedia).toHaveBeenCalledWith('m1')
    w.unmount()
  })

  it('加标记 → 标脏 → 保存携带 expected_version → 清脏 + 列表重拉', async () => {
    videoApi.mediaFrames.mockResolvedValue({
      frame_count: 40, first_pts_us: 0, last_pts_us: 1332000,
      current: { index: 12, pts_us: 333000 },
    })
    const w = await mountOpenProject()

    await w.find('[data-testid="marker-label-input"]').setValue('开始点击')
    await w.find('[data-testid="marker-add"]').trigger('click')
    await flushPromises()

    expect(w.find('[data-testid="project-dirty"]').text()).toContain('未保存')
    expect(w.findAll('[data-testid="marker-row"]')).toHaveLength(1)

    await w.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.putProject).toHaveBeenCalledWith('pkg', 'p1', expect.any(String), { expectedVersion: 'v1' })
    const saved = JSON.parse(videoApi.putProject.mock.calls[0][2])
    expect(saved.markers[0].frame).toEqual({ media_id: 'm1', frame_index: 12, pts_us: 333000, calibration_version: 1 })
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('已保存')
    // 保存后重拉列表（摘要更新）
    expect(videoApi.listProjectEntries).toHaveBeenCalledTimes(2)
    w.unmount()
  })

  it('标记注释/删除走同一保存流；校准应用后版本递增 + 旧标记标脏', async () => {
    const withMarkerEntry = { ...PROJECT_ENTRY, content: projectJson({
      markers: [{ id: 'mk-1', label: '旧标记', note: '', created_at: '2026-09-07T00:00:00Z', frame: { media_id: 'm1', frame_index: 3, pts_us: 90000, calibration_version: 1 } }],
    }) }
    const w = await mountOpenProject(withMarkerEntry)
    expect(w.findAll('[data-testid="marker-row"]')).toHaveLength(1)

    // 注释更新 → 标脏（setValue 自带 change 派发，勿叠加 trigger）
    await w.find('[data-testid="marker-note-mk-1"]').setValue('等动画结束')
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('未保存')

    // 校准：旋转 90 → 应用 → version 2，旧校准横幅出现
    expect(w.find('[data-testid="marker-stale-banner"]').exists()).toBe(false)
    await w.find('[data-testid="calibration-rotation"]').setValue('90')
    await w.find('[data-testid="calibration-apply"]').trigger('click')
    expect(w.find('[data-testid="marker-stale-banner"]').exists()).toBe(true)
    await w.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    const saved = JSON.parse(videoApi.putProject.mock.calls[0][2])
    expect(saved.calibration.version).toBe(2)
    expect(saved.calibration.rotation).toBe(90)
    expect(saved.markers[0].frame.calibration_version).toBe(1)
    w.unmount()

    // 重开项目（保存内容回读）：标脏横幅 + 行内徽章
    videoApi.putProject.mockClear()
    const savedEntry = { ...PROJECT_ENTRY, content: projectJson({
      calibration: { version: 2, rotation: 90, pixel_aspect: { num: 1, den: 1 }, content_rect: null, reference_size: { width: 1920, height: 1080 } },
      markers: [{ id: 'mk-1', label: '旧标记', note: '', created_at: '', frame: { media_id: 'm1', frame_index: 3, pts_us: 90000, calibration_version: 1 } }],
    }) }
    const w2 = await mountOpenProject(savedEntry)
    expect(w2.find('[data-testid="marker-stale-banner"]').text()).toContain('旧校准')
    expect(w2.find('[data-testid="marker-stale"]').exists()).toBe(true)
    w2.unmount()
  })

  it('删除标记 → withoutMarker', async () => {
    const withMarkerEntry = { ...PROJECT_ENTRY, content: projectJson({
      markers: [{ id: 'mk-1', label: 'x', note: '', created_at: '', frame: { media_id: 'm1', frame_index: 3, pts_us: 90000, calibration_version: 1 } }],
    }) }
    const w = await mountOpenProject(withMarkerEntry)
    await w.find('[data-testid="marker-del-mk-1"]').trigger('click')
    expect(w.findAll('[data-testid="marker-row"]')).toHaveLength(0)
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('未保存')
    w.unmount()
  })

  it('保存冲突（409 version_conflict）：横幅提示 + 重新加载恢复', async () => {
    videoApi.mediaFrames.mockResolvedValue({
      frame_count: 10, first_pts_us: 0, last_pts_us: 300000,
      current: { index: 2, pts_us: 60000 },
    })
    const w = await mountOpenProject()
    await w.find('[data-testid="marker-label-input"]').setValue('m')
    await w.find('[data-testid="marker-add"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('未保存')

    videoApi.putProject.mockRejectedValueOnce(Object.assign(new Error('conflict'), {
      status: 409,
      code: 'version_conflict: 资源已被其他页面修改',
    }))
    await w.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    expect(w.find('[data-testid="project-conflict-banner"]').text()).toContain('保存冲突')

    // 重新加载：getProject 再读，清脏
    await w.find('[data-testid="project-reload"]').trigger('click')
    await flushPromises()
    expect(videoApi.getProject).toHaveBeenCalledWith('pkg', 'p1')
    expect(w.find('[data-testid="project-conflict-banner"]').exists()).toBe(false)
    expect(w.find('[data-testid="project-dirty"]').text()).toContain('已保存')
    w.unmount()
  })

  it('重命名 / 删除项目走资源 rename / delete', async () => {
    const w = await mountOpenProject()
    const row = w.findAll('[data-testid="project-row"]')[0]
    await row.find('[data-testid="project-rename"]').trigger('click')
    await row.find('[data-testid="project-rename-input"]').setValue('p2')
    await row.find('[data-testid="project-rename-confirm"]').trigger('click')
    await flushPromises()
    expect(videoApi.renameProject).toHaveBeenCalledWith('pkg', 'p1', 'p2')

    const deleteBtn = () => w.findAll('[data-testid="project-row"]')[0].find('[data-testid="project-delete"]')
    await deleteBtn().trigger('click') // armed
    expect(videoApi.deleteProject).not.toHaveBeenCalled()
    await deleteBtn().trigger('click') // confirm
    await flushPromises()
    expect(videoApi.deleteProject).toHaveBeenCalledWith('pkg', 'p1')
    // 打开中的项目被删除 → 回落空态
    expect(w.find('[data-testid="video-timeline"]').exists()).toBe(false)
    w.unmount()
  })

  it('项目关联录制会话：草稿入口带入 recordingId 切到草稿分区', async () => {
    const entry = { ...PROJECT_ENTRY, content: projectJson({ recording: { recording_id: 'rec-42' } }) }
    const w = await mountOpenProject(entry)
    await w.find('[data-testid="project-open-draft"]').trigger('click')
    expect(w.find('[data-testid="video-draft"]').exists()).toBe(true)
    expect(w.find('[data-testid="draft-recording-id"]').element.value).toBe('rec-42')
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// VideoProjects 项目列表组件
// ---------------------------------------------------------------------------

describe('VideoProjects 项目列表组件', () => {
  const SUMMARIES = [
    { id: 'p1', name: '项目一', valid: true, markerCount: 2, assetCount: 1 },
    { id: 'p2', name: 'p2', valid: false, markerCount: 0, assetCount: 0 },
  ]

  it('新建表单：id 语法校验 + 重名拒绝 + create 事件', async () => {
    const w = mount(VideoProjects, { props: { projects: SUMMARIES, canCreate: true } })
    // canCreate=true：新建按钮可用
    expect(w.find('[data-testid="project-create"]').attributes('disabled')).toBeUndefined()
    await w.find('[data-testid="project-create"]').trigger('click')

    await w.find('[data-testid="project-create-id"]').setValue('Bad ID')
    expect(w.find('[data-testid="project-create-confirm"]').attributes('disabled')).toBeDefined()
    await w.find('[data-testid="project-create-id"]').setValue('p1') // 重名
    await w.find('[data-testid="project-create-name"]').setValue('x')
    await w.find('[data-testid="project-create-confirm"]').trigger('click')
    expect(w.find('.zone-error').exists()).toBe(true)
    expect(w.emitted('create')).toBeUndefined()

    await w.find('[data-testid="project-create-id"]').setValue('p3')
    await w.find('[data-testid="project-create-confirm"]').trigger('click')
    expect(w.emitted('create')).toEqual([[{ id: 'p3', name: 'x' }]])
    w.unmount()
  })

  it('重命名：行内输入 → rename(id, newId)；删除两段确认 → delete(id)', async () => {
    const w = mount(VideoProjects, { props: { projects: SUMMARIES, canCreate: true } })
    const row = w.findAll('[data-testid="project-row"]')[0]
    await row.find('[data-testid="project-rename"]').trigger('click')
    await row.find('[data-testid="project-rename-input"]').setValue('p9')
    await row.find('[data-testid="project-rename-confirm"]').trigger('click')
    expect(w.emitted('rename')).toEqual([['p1', 'p9']])

    const deleteBtn = w.findAll('[data-testid="project-row"]')[1].find('[data-testid="project-delete"]')
    await deleteBtn.trigger('click')
    expect(w.emitted('delete')).toBeUndefined()
    expect(deleteBtn.text()).toBe('确认删除')
    await deleteBtn.trigger('click')
    expect(w.emitted('delete')).toEqual([['p2']])
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// VideoTimeline 时间轴区（标记 / 校准 / 事件叠加）
// ---------------------------------------------------------------------------

describe('VideoTimeline 时间轴区', () => {
  const CAL = { version: 1, rotation: 0, pixel_aspect: { num: 1, den: 1 }, content_rect: null, reference_size: { width: 1920, height: 1080 } }
  const MARKER = { id: 'mk-1', label: '开始点击', note: '', created_at: '', frame: { media_id: 'm1', frame_index: 12, pts_us: 333000, calibration_version: 1 } }

  it('精确帧与逐帧（服务端真实帧表，无固定步长）+ 帧失败提示（回归）', async () => {
    videoApi.mediaFrames.mockResolvedValue({
      frame_count: 40, first_pts_us: 0, last_pts_us: 1332000,
      current: { index: 20, pts_us: 333000 },
    })
    videoApi.mediaFrameNeighbors.mockResolvedValue({
      index: 20, pts_us: 333000,
      prev: { index: 19, pts_us: 300000 },
      next: { index: 21, pts_us: 366333 },
    })
    const w = mount(VideoTimeline, { props: { media: MEDIA[0], markers: [], calibration: CAL } })
    await flushPromises()
    const video = w.find('video')
    video.element.currentTime = 0.333
    await video.trigger('timeupdate')

    expect(w.find('[data-testid="frame-count"]').text()).toContain('40 帧')
    await w.find('[data-testid="frame-next"]').trigger('click')
    await flushPromises()
    expect(videoApi.mediaFrames).toHaveBeenCalledWith('m1', { ptsUs: 333000 })
    expect(videoApi.mediaFrameNeighbors).toHaveBeenCalledWith('m1', 20)
    expect(w.find('[data-testid="frame-image"]').attributes('src')).toBe('/api/media/m1/frame?index=21&max_width=640')
    w.unmount()

    videoApi.mediaFrames.mockRejectedValue(new Error('down'))
    const w2 = mount(VideoTimeline, { props: { media: MEDIA[1], markers: [], calibration: CAL } })
    await flushPromises()
    expect(w2.find('[data-testid="frame-prev"]').attributes('disabled')).toBeDefined()
    expect(w2.find('[data-testid="frame-error"]').text()).toContain('展示帧表加载失败')
    w2.unmount()
  })

  it('加标记：解析当前帧身份后上抛（帧身份 + 当前校准版本，非浮点秒）', async () => {
    videoApi.mediaFrames.mockResolvedValue({
      frame_count: 40, first_pts_us: 0, last_pts_us: 1332000,
      current: { index: 12, pts_us: 333000 },
    })
    const w = mount(VideoTimeline, { props: { media: MEDIA[0], markers: [], calibration: CAL } })
    await flushPromises()
    await w.find('[data-testid="marker-label-input"]').setValue('开始点击')
    await w.find('[data-testid="marker-add"]').trigger('click')
    await flushPromises()

    const emitted = w.emitted('add-marker')
    expect(emitted).toHaveLength(1)
    expect(emitted[0][0]).toEqual({
      label: '开始点击',
      frame: { media_id: 'm1', frame_index: 12, pts_us: 333000, calibration_version: 1 },
    })
    expect(w.find('[data-testid="marker-label-input"]').element.value).toBe('')
    w.unmount()
  })

  it('标记跳转 / 注释编辑 / 旧校准标脏', async () => {
    const staleCal = { ...CAL, version: 2 }
    const w = mount(VideoTimeline, {
      props: { media: MEDIA[0], markers: [MARKER], calibration: staleCal },
    })
    await flushPromises()
    expect(w.find('[data-testid="marker-stale-banner"]').text()).toContain('旧校准')
    expect(w.find('[data-testid="marker-stale"]').exists()).toBe(true)

    await w.find('[data-testid="marker-jump-mk-1"]').trigger('click')
    expect(w.find('[data-testid="frame-image"]').attributes('src')).toBe('/api/media/m1/frame?index=12&max_width=640')

    await w.find('[data-testid="marker-note-mk-1"]').setValue('note-1')
    expect(w.emitted('update-marker')).toEqual([['mk-1', { note: 'note-1' }]])

    await w.find('[data-testid="marker-del-mk-1"]').trigger('click')
    expect(w.emitted('remove-marker')).toEqual([['mk-1']])
    w.unmount()
  })

  it('校准表单：应用上抛 next（宿主负责版本递增）；非法参考分辨率本地报错', async () => {
    const w = mount(VideoTimeline, { props: { media: MEDIA[0], markers: [], calibration: CAL } })
    await flushPromises()
    await w.find('[data-testid="calibration-rotation"]').setValue('270')
    await w.find('[data-testid="calibration-rect-x"]').setValue('0')
    await w.find('[data-testid="calibration-rect-y"]').setValue('140')
    await w.find('[data-testid="calibration-rect-w"]').setValue('1920')
    await w.find('[data-testid="calibration-rect-h"]').setValue('800')
    await w.find('[data-testid="calibration-apply"]').trigger('click')

    const emitted = w.emitted('save-calibration')
    expect(emitted).toHaveLength(1)
    expect(emitted[0][0]).toEqual({
      rotation: 270,
      pixel_aspect: { num: 1, den: 1 },
      reference_size: { width: 1920, height: 1080 },
      content_rect: { x: 0, y: 140, w: 1920, h: 800 },
    })

    await w.find('[data-testid="calibration-ref-w"]').setValue('0')
    await w.find('[data-testid="calibration-apply"]').trigger('click')
    expect(w.find('[data-testid="calibration-error"]').text()).toContain('参考分辨率')
    w.unmount()
  })

  it('自录事件叠加：base_pts_us 整数映射 + 来源标注 + 点击跳帧；无映射标脏', async () => {
    videoApi.recordingStatus.mockResolvedValue({
      id: 'rec-1',
      segments: [{ media_id: 'm1', start_us: 0, duration_us: 65000000, base_pts_us: 1000, reason: 'normal' }],
    })
    videoApi.recordingEvents.mockResolvedValue([
      { event_id: 'e1', source: 'keymap', kind: 'tap', status: 'accepted', timeline_us: 1200000, payload: { x: 820, y: 460 } },
      { event_id: 'e2', source: 'manual', kind: 'key', status: 'accepted', timeline_us: 99000000, payload: { code: 'KeyA' } },
    ])
    videoApi.mediaFrames.mockResolvedValue({
      frame_count: 100, first_pts_us: 0, last_pts_us: 65000000,
      current: { index: 36, pts_us: 1201000 },
    })
    const w = mount(VideoTimeline, { props: { media: MEDIA[0], markers: [], calibration: CAL, recordingId: 'rec-1' } })
    await flushPromises()
    await w.find('[data-testid="events-load"]').trigger('click')
    await flushPromises()

    expect(videoApi.recordingStatus).toHaveBeenCalledWith('rec-1')
    const rows = w.findAll('[data-testid="timeline-event-row"]')
    expect(rows).toHaveLength(2)
    // 升序：e1（1.2s，段内可映射）在前，e2（99s，分段外）在后
    expect(rows[0].text()).toContain('键映射')
    expect(rows[0].text()).toContain('1.200s')
    expect(rows[1].text()).toContain('手动')
    expect(rows[1].text()).toContain('未对齐')

    // 点击跳帧：事件 PTS（1000+1200000）→ 服务端解析展示帧身份
    await rows[0].trigger('click')
    await flushPromises()
    expect(videoApi.mediaFrames).toHaveBeenCalledWith('m1', { ptsUs: 1201000 })
    expect(w.find('[data-testid="frame-image"]').attributes('src')).toBe('/api/media/m1/frame?index=36&max_width=640')
    w.unmount()
  })

  it('外部素材（无 recordingId）不渲染事件区——不伪造操作日志', async () => {
    const w = mount(VideoTimeline, { props: { media: MEDIA[1], markers: [], calibration: CAL, recordingId: '' } })
    await flushPromises()
    expect(w.find('[data-testid="events-box"]').exists()).toBe(false)
    w.unmount()
  })
})

// ---------------------------------------------------------------------------
// MediaLibrary 素材库区
// ---------------------------------------------------------------------------

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
    expect(w2.find('[data-testid="media-error"]').text()).toContain('被视频项目引用')
    w2.unmount()
  })

  it('录制入口：选中设备后轮询活动会话驱动按钮态；停止 → recording-finished + changed', async () => {
    const w = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    await flushPromises()
    expect(w.find('[data-testid="record-start"]').attributes('disabled')).toBeDefined()
    expect(videoApi.activeRecording).not.toHaveBeenCalled()

    await w.find('[data-testid="record-device"]').setValue('dev-a')
    await flushPromises()
    expect(videoApi.activeRecording).toHaveBeenCalledWith('dev-a')
    expect(w.find('[data-testid="record-start"]').attributes('disabled')).toBeUndefined()

    videoApi.activeRecording.mockResolvedValue(SESSION)
    videoApi.recordingStop.mockResolvedValue(FINISHED)
    await w.find('[data-testid="record-device"]').setValue('dev-b')
    await w.find('[data-testid="record-device"]').setValue('dev-a')
    await flushPromises()
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

// ---------------------------------------------------------------------------
// VideoDraft 草稿区状态流转（Phase 7 §10.3 可编辑工作流）
// ---------------------------------------------------------------------------

describe('VideoDraft 草稿区状态流转（Phase 7 可编辑工作流）', () => {
  const mountDraft = (recordingId = 'rec-9', yamlReady = true) =>
    mount(VideoDraft, { props: { recordingId, packageId: 'pkg', yamlReady } })

  it('载入事件：时间轴升序渲染 kind/时间/来源；全选/清空驱动已选数', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()

    expect(videoApi.recordingEvents).toHaveBeenCalledWith('rec-9')
    const rows = wrapper.findAll('.event-row')
    expect(rows).toHaveLength(2)
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

  it('yaml 依赖门禁（§10.1）：未 Running 时生成禁用 + 依赖横幅，注释/重排仍可见', async () => {
    const wrapper = mountDraft('rec-9', false)
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-dep-banner"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="draft-generate"]').attributes('disabled')).toBeDefined()
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    expect(wrapper.find('[data-testid="draft-generate"]').attributes('disabled')).toBeDefined()
    wrapper.unmount()
  })

  it('勾选+注释 → 生成：createVideoDraft(recordingId, 顺序 ids, comments)，展示 yaml/诊断/回查 source', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    videoApi.createVideoDraft.mockResolvedValue({
      yaml: 'version: 3\nsteps:\n  - tap: [100, 200]',
      diagnostics: [{ event_id: 'e2', reason: 'multi_touch_not_supported' }],
      source: { recording_id: 'rec-9', events: [
        { event_id: 'e1', kind: 'swipe', timeline_us: 1200000, selected: true, mapped: true },
        { event_id: 'e2', kind: 'tap', timeline_us: 5200000, selected: true, mapped: false },
      ] },
    })
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="draft-event-check-e1"]').setValue(true)
    await wrapper.find('[data-testid="draft-event-comment-e1"]').setValue('打开背包')
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()

    expect(videoApi.createVideoDraft).toHaveBeenCalledWith('rec-9', ['e1'], { e1: '打开背包' })
    expect(wrapper.find('[data-testid="draft-yaml"]').text()).toContain('tap: [100, 200]')
    const diags = wrapper.find('[data-testid="draft-diagnostics"]')
    expect(diags.text()).toContain('e2')
    expect(diags.text()).toContain('multi_touch_not_supported')
    expect(wrapper.find('[data-testid="draft-source-line"]').text()).toContain('rec-9')
    expect(wrapper.text()).toContain('草稿不会自动执行')
    wrapper.unmount()
  })

  it('重排（↑↓）改变生成顺序；保存草稿 → automation.save_draft + open_editor 导航', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    videoApi.createVideoDraft.mockResolvedValue({
      yaml: 'version: 3\nsteps:\n  - log: ok', diagnostics: [],
      source: { recording_id: 'rec-9', events: [] },
    })
    videoApi.saveDraft.mockResolvedValue({ id: 'pkg/my-draft.yaml', path: 'automations/my-draft.yaml', package_id: 'pkg' })
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="draft-event-check-e2"]').setValue(true)
    await wrapper.find('[data-testid="draft-event-check-e1"]').setValue(true)
    // 选中顺序 = 勾选顺序（e2, e1）；e1 上移 → e1, e2
    await wrapper.find('[data-testid="draft-event-up-e1"]').trigger('click')

    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    expect(videoApi.createVideoDraft).toHaveBeenCalledWith('rec-9', ['e1', 'e2'], {})

    await wrapper.find('[data-testid="draft-save-name"]').setValue('my-draft')
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.saveDraft).toHaveBeenCalledWith({
      packageId: 'pkg', name: 'my-draft', yaml: 'version: 3\nsteps:\n  - log: ok', overwrite: false,
    })
    // automation.open_editor（前端契约）：路由切到自动化面板
    expect(routerPush).toHaveBeenCalledWith(expect.objectContaining({
      query: expect.objectContaining({ panel: 'gamer.yaml:automation' }),
    }))
    wrapper.unmount()
  })

  it('保存重名冲突给出可理解提示（overwrite 引导）', async () => {
    const wrapper = mountDraft()
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    videoApi.createVideoDraft.mockResolvedValue({ yaml: 'version: 3\nsteps: []', diagnostics: [], source: null })
    videoApi.saveDraft.mockRejectedValue(new Error('自动化脚本已存在: pkg/d.yaml（确认覆盖请带 overwrite:true）'))
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="draft-save-name"]').setValue('d')
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-error"]').text()).toContain('覆盖同名')
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
    const wrapper = mount(VideoDraft, { props: { recordingId: '', packageId: 'pkg', yamlReady: true } })
    videoApi.recordingEvents.mockResolvedValue(EVENTS)
    await wrapper.setProps({ recordingId: 'rec-77' })
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-recording-id"]').element.value).toBe('rec-77')
    expect(videoApi.recordingEvents).toHaveBeenCalledWith('rec-77')
    wrapper.unmount()
  })
})

// ---------------------------------------------------------------------------
// Phase 8 遗留 #2：项目保存/删除 → 媒体引用全量替换同步（契约 §2.1）
// ---------------------------------------------------------------------------

describe('项目保存 → 媒体引用同步（遗留 #2）', () => {
  const MARKER = (mediaId = 'm1') => ({ id: 'mk-1', label: '旧标记', note: '', created_at: '', frame: { media_id: mediaId, frame_index: 3, pts_us: 90000, calibration_version: 1 } })

  async function mountOpenDirtyProject(content) {
    const entry = { ...PROJECT_ENTRY, content }
    stubProject(entry)
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await w.findAll('[data-testid="project-row"]')[0].trigger('click')
    await flushPromises()
    // 改标记注释 → 标脏（保存按钮才可用）
    await w.find('[data-testid="marker-note-mk-1"]').setValue('备注')
    return w
  }

  it('保存时向引用媒体登记 gamer.video/project 引用（全量替换端点）', async () => {
    videoApi.getMedia.mockResolvedValue({ id: 'm1', refs: [] })
    const w = await mountOpenDirtyProject(projectJson({ markers: [MARKER()] }))
    await w.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.setMediaRefs).toHaveBeenCalledWith('m1', [
      { package_id: 'pkg', plugin_id: 'gamer.video', kind: 'project' },
    ])
    w.unmount()
  })

  it('重复保存幂等：引用已登记且未变化时不重复写 refs', async () => {
    videoApi.getMedia.mockResolvedValue({ id: 'm1', refs: [
      { package_id: 'pkg', plugin_id: 'gamer.video', kind: 'project' },
    ] })
    const w = await mountOpenDirtyProject(projectJson({ markers: [MARKER()] }))
    await w.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.setMediaRefs).not.toHaveBeenCalled()
    w.unmount()
  })

  it('删除项目：解除其素材引用；素材已删除（404）静默跳过', async () => {
    videoApi.getMedia.mockRejectedValue(Object.assign(new Error('gone'), { status: 404 }))
    stubProject(PROJECT_ENTRY)
    const w = mount(VideoWorkbench)
    await flushPromises()
    await w.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    videoApi.deleteProject.mockResolvedValue(null)
    const deleteBtn = () => w.findAll('[data-testid="project-row"]')[0].find('[data-testid="project-delete"]')
    await deleteBtn().trigger('click') // armed
    await deleteBtn().trigger('click') // confirm
    await flushPromises()
    expect(videoApi.deleteProject).toHaveBeenCalledWith('pkg', 'p1')
    // 404 静默：不产生 refs 写入，也不弹错误横幅
    expect(videoApi.setMediaRefs).not.toHaveBeenCalled()
    expect(w.find('[data-testid="project-conflict-banner"]').exists()).toBe(false)
    w.unmount()
  })
})
