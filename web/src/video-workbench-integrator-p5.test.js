// @vitest-environment happy-dom
/**
 * P5-WB-INTEGRATOR：只验证 VideoWorkbench 父装配。
 * 子组件和 videoApi 的专属行为在各自测试中覆盖；这里只检查真实事件是否
 * 穿过父组件、进入既有保存/ref 同步路径，以及上下文/未保存保护是否生效。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const routerPush = vi.fn(async () => {})

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: routerPush, currentRoute: { value: { query: {} } } }),
}))

vi.mock('./api', () => ({
  api: { listTemplates: vi.fn(async () => []) },
}))

vi.mock('./components/console/useConsoleStage', async importOriginal => {
  const actual = await importOriginal()
  return { ...actual, requestStageMedia: vi.fn() }
})

vi.mock('./components/video/videoApi', async importOriginal => {
  const actual = await importOriginal()
  return {
    ...actual,
    videoApi: {
      ...actual.videoApi,
      listMedia: vi.fn(),
      listProjectEntries: vi.fn(),
      getProject: vi.fn(),
      putProject: vi.fn(),
      getMedia: vi.fn(),
      setMediaRefs: vi.fn(),
      recordingEvents: vi.fn(),
      gamerYamlCapabilities: vi.fn(),
      createVideoDraft: vi.fn(),
      saveDraft: vi.fn(),
    },
  }
})

import VideoWorkbench from './components/video/VideoWorkbench.vue'
import { requestStageMedia } from './components/console/useConsoleStage'
import { videoApi } from './components/video/videoApi'
import { devicesData, store } from './store'
import { packageStore } from './package-store'

const MEDIA_A = {
  id: 'media-a', name: '录制 A', source: 'recording', state: 'ready',
  recording_id: 'recording-a', device_id: 'device-a', duration_us: 2e6,
  width: 100, height: 50, sha256: 'a'.repeat(64),
}
const MEDIA_B = {
  id: 'media-b', name: '素材 B', source: 'import', state: 'ready',
  duration_us: 3e6, width: 200, height: 100, sha256: 'b'.repeat(64),
}

function projectJson(id = 'project-a', name = '项目 A', mediaId = 'media-a') {
  return JSON.stringify({
    schema_version: 1,
    id,
    name,
    package_id: 'pkg-a',
    notes: '',
    created_at: '2026-09-09T00:00:00Z',
    updated_at: '2026-09-09T00:00:00Z',
    assets: [{ media_id: mediaId, role: 'primary', sha256: 'a'.repeat(64), duration_us: 2e6, frame_count: null }],
    recording: null,
    calibration: {
      version: 1, rotation: 0, pixel_aspect: { num: 1, den: 1 },
      content_rect: null, reference_size: { width: 100, height: 50 },
    },
    markers: [],
    progress: { stage: 'created', updated_at: '2026-09-09T00:00:00Z' },
  })
}

function entry(id = 'project-a', name = '项目 A') {
  return { path: `projects/${id}.json`, content: projectJson(id, name), version: `version-${id}` }
}

function installDefaults() {
  store.deviceId = 'device-a'
  devicesData.value = [{ id: 'device-a', name: '设备 A', pkg: 'com.example.game' }]
  packageStore.packages = [{ id: 'pkg-a' }, { id: 'pkg-b' }]
  packageStore.currentPackageId = 'pkg-a'
  videoApi.listMedia.mockResolvedValue([MEDIA_A, MEDIA_B])
  videoApi.listProjectEntries.mockResolvedValue([])
  videoApi.getProject.mockResolvedValue(entry())
  videoApi.putProject.mockResolvedValue({ ok: true, version: 'version-next' })
  videoApi.getMedia.mockResolvedValue({ id: 'media-a', refs: [] })
  videoApi.setMediaRefs.mockResolvedValue({})
  videoApi.gamerYamlCapabilities.mockResolvedValue({
    id: 'gamer.yaml', state: 'running', running: true,
    actions: [
      { action: 'automation.create_draft', version: '1' },
      { action: 'automation.save_draft', version: '1' },
    ],
  })
  videoApi.recordingEvents.mockResolvedValue([{
    event_id: 'event-a', kind: 'tap', source: 'manual', status: 'accepted',
    timeline_us: 10, payload: { x: 1, y: 2 },
  }])
  videoApi.createVideoDraft.mockResolvedValue({
    yaml: 'run:\n  - tap: [1, 2]\n', diagnostics: [],
    source: { recording_id: 'recording-a', events: [] },
  })
  videoApi.saveDraft.mockResolvedValue({ id: 'pkg-a/automations/from-video.yaml', package_id: 'pkg-a' })
}

beforeEach(() => {
  vi.clearAllMocks()
  installDefaults()
})

async function mountWorkbench(projectEntries) {
  if (projectEntries !== undefined) videoApi.listProjectEntries.mockResolvedValue(projectEntries)
  const wrapper = mount(VideoWorkbench)
  await flushPromises()
  return wrapper
}

describe('P5-WB-INTEGRATOR VideoWorkbench 父装配', () => {
  it('素材列表失败保留旧数据并提供重试；录制历史选择进入草稿并传递 Device/App/Package', async () => {
    const wrapper = await mountWorkbench()
    expect(wrapper.findAll('[data-testid="media-row"]')).toHaveLength(2)

    videoApi.listMedia.mockRejectedValueOnce(new Error('网络不可用'))
    await wrapper.find('[data-testid="media-refresh"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="media-load-error"]').text()).toContain('网络不可用')
    expect(wrapper.findAll('[data-testid="media-row"]')).toHaveLength(2)

    videoApi.listMedia.mockResolvedValueOnce([MEDIA_A, MEDIA_B])
    await wrapper.find('[data-testid="media-retry"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="recording-history-row"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="video-draft"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="draft-context"]').text()).toContain('Package：pkg-a')
    expect(wrapper.find('[data-testid="draft-context"]').text()).toContain('设备：device-a')
    expect(wrapper.find('[data-testid="draft-context"]').text()).toContain('Android：com.example.game')
    expect(videoApi.recordingEvents).toHaveBeenCalledWith('recording-a')
    wrapper.unmount()
  })

  it('项目素材事件进入本地编辑和显式 expected_version 保存，保存后复用全量媒体引用同步', async () => {
    videoApi.listProjectEntries.mockResolvedValue([entry()])
    const wrapper = await mountWorkbench()
    await wrapper.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="project-row"]').trigger('click')
    await flushPromises()
    requestStageMedia.mockClear()

    await wrapper.find('[data-testid="asset-target"]').setValue('media-b')
    await wrapper.find('[data-testid="asset-add"]').trigger('click')
    expect(wrapper.find('[data-testid="project-dirty"]').text()).toContain('未保存')
    expect(wrapper.findAll('[data-testid="project-asset-row"]')).toHaveLength(2)
    expect(requestStageMedia).not.toHaveBeenCalled()

    videoApi.getMedia.mockImplementation(async id => ({ id, refs: [] }))
    await wrapper.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.putProject).toHaveBeenCalledWith(
      'pkg-a', 'project-a', expect.any(String), { expectedVersion: 'version-project-a' },
    )
    expect(JSON.parse(videoApi.putProject.mock.calls[0][2]).assets.map(asset => asset.media_id))
      .toEqual(['media-a', 'media-b'])
    expect(videoApi.setMediaRefs).toHaveBeenCalledWith('media-a', [
      { package_id: 'pkg-a', plugin_id: 'gamer.video', kind: 'project' },
    ])
    expect(videoApi.setMediaRefs).toHaveBeenCalledWith('media-b', [
      { package_id: 'pkg-a', plugin_id: 'gamer.video', kind: 'project' },
    ])
    wrapper.unmount()
  })

  it('打开素材切换 Stage；切换项目或 Package 时保护未保存项目副本', async () => {
    videoApi.listProjectEntries.mockResolvedValue([entry('project-a', '项目 A'), entry('project-b', '项目 B')])
    videoApi.getProject.mockImplementation(async (_pkg, id) => entry(id, id === 'project-a' ? '项目 A' : '项目 B'))
    const wrapper = await mountWorkbench()
    await wrapper.find('[data-testid="workbench-tab-projects"]').trigger('click')
    await flushPromises()
    await wrapper.findAll('[data-testid="project-row"]')[0].trigger('click')
    await flushPromises()

    await wrapper.find('[data-testid="asset-view"]').trigger('click')
    expect(requestStageMedia).toHaveBeenCalledWith('media-a')
    await wrapper.find('[data-testid="asset-target"]').setValue('media-b')
    await wrapper.find('[data-testid="asset-add"]').trigger('click')
    await wrapper.findAll('[data-testid="project-row"]')[1].trigger('click')
    expect(wrapper.find('[data-testid="project-switch-protect"]').exists()).toBe(true)
    expect(videoApi.getProject).toHaveBeenCalledTimes(1)

    await wrapper.find('[data-testid="project-switch-discard"]').trigger('click')
    await flushPromises()
    expect(videoApi.getProject).toHaveBeenLastCalledWith('pkg-a', 'project-b')

    // Package 切换同样不能把未保存副本写进新作用域。
    await wrapper.find('[data-testid="asset-target"]').setValue('media-b')
    await wrapper.find('[data-testid="asset-add"]').trigger('click')
    packageStore.currentPackageId = 'pkg-b'
    await flushPromises()
    expect(wrapper.find('[data-testid="package-switch-protect"]').exists()).toBe(true)
    expect(packageStore.currentPackageId).toBe('pkg-a')
    await wrapper.find('[data-testid="package-switch-discard"]').trigger('click')
    await flushPromises()
    expect(packageStore.currentPackageId).toBe('pkg-b')
    wrapper.unmount()
  })

  it('草稿生成、保存和打开编辑器沿用来源 Package，不要求手工输入 recording id', async () => {
    const wrapper = await mountWorkbench()
    await wrapper.find('[data-testid="recording-history-row"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="draft-save-name"]').setValue('from-video')
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.createVideoDraft).toHaveBeenCalledWith('recording-a', ['event-a'], {})
    expect(videoApi.saveDraft).toHaveBeenCalledWith({
      packageId: 'pkg-a', name: 'from-video', yaml: 'run:\n  - tap: [1, 2]\n', overwrite: false,
    })
    expect(routerPush).toHaveBeenCalledWith(expect.objectContaining({
      query: expect.objectContaining({ panel: 'gamer.yaml:automation' }),
    }))
    wrapper.unmount()
  })
})
