// @vitest-environment happy-dom
/**
 * P1-VREF 专属回归：验证 VideoWorkbench ↔ videoApi 的媒体引用契约、
 * 成功 PUT 后的项目状态计算，以及保存成功与引用同步失败/重试的状态边界。
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const fetchStub = vi.fn()

vi.mock('./components/video/videoApi', async (importOriginal) => {
  const actual = await importOriginal()
  return {
    ...actual,
    videoApi: {
      ...actual.videoApi,
      listMedia: vi.fn(),
      activeRecording: vi.fn(),
      listProjectEntries: vi.fn(),
      getProject: vi.fn(),
      putProject: vi.fn(),
      getMedia: vi.fn(),
      // 保留真实 setMediaRefs，直接验证 Workbench 传入的 REST 字段能落成正确请求。
      mediaFrames: vi.fn(),
      mediaFrameNeighbors: vi.fn(),
    },
  }
})

vi.mock('./components/console/useConsoleStage', async (importOriginal) => {
  const actual = await importOriginal()
  return { ...actual, requestStageMedia: vi.fn() }
})

vi.mock('./components/video/yamlCapability', () => ({
  useYamlCapability: () => ({
    ready: true,
    start: vi.fn(),
    stop: vi.fn(),
  }),
}))

import VideoWorkbench from './components/video/VideoWorkbench.vue'
import { videoApi } from './components/video/videoApi'
import { packageStore } from './package-store'

function jsonResponse(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => String(name).toLowerCase() === 'content-type' ? 'application/json' : null },
    json: async () => body,
  }
}

function projectJson(mediaId, note = '') {
  return JSON.stringify({
    schema_version: 1,
    id: 'p1',
    name: '引用回归项目',
    package_id: 'pkg.scope',
    notes: '',
    created_at: '2026-09-07T00:00:00Z',
    updated_at: '2026-09-07T00:00:00Z',
    assets: [{ media_id: mediaId, role: 'primary', sha256: '', duration_us: 1000000, frame_count: null }],
    recording: null,
    calibration: {
      version: 1,
      rotation: 0,
      pixel_aspect: { num: 1, den: 1 },
      content_rect: null,
      reference_size: { width: 1920, height: 1080 },
    },
    markers: [{
      id: 'mk-1',
      label: '标记',
      note,
      created_at: '2026-09-07T00:00:00Z',
      frame: { media_id: mediaId, frame_index: 1, pts_us: 40000, calibration_version: 1 },
    }],
    progress: { stage: 'markers', updated_at: '2026-09-07T00:00:00Z' },
  })
}

function entry(mediaId) {
  return { path: 'projects/p1.json', content: projectJson(mediaId), version: 'v1' }
}

async function openDirtyProject({ savedMediaId = 'm2', listedMediaId = 'm1', afterListedMediaId = savedMediaId } = {}) {
  const listedBefore = entry(listedMediaId)
  const listedAfter = entry(afterListedMediaId)
  const opened = entry(savedMediaId)
  videoApi.listProjectEntries.mockResolvedValueOnce([listedBefore]).mockResolvedValue([listedAfter])
  videoApi.getProject.mockResolvedValue(opened)
  videoApi.putProject.mockResolvedValue({ ok: true, version: 'v2' })
  videoApi.getMedia.mockResolvedValue({
    id: savedMediaId,
    refs: [{ package_id: 'other.pkg', plugin_id: 'other.plugin', kind: 'workspace' }],
  })

  const wrapper = mount(VideoWorkbench)
  await flushPromises()
  await wrapper.find('[data-testid="workbench-tab-projects"]').trigger('click')
  await flushPromises()
  await wrapper.find('[data-testid="project-row"]').trigger('click')
  await flushPromises()
  await wrapper.find('[data-testid="marker-note-mk-1"]').setValue('已编辑')
  return wrapper
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal('fetch', fetchStub)
  packageStore.currentPackageId = 'pkg.scope'
  videoApi.listMedia.mockResolvedValue([
    { id: 'm1', name: 'before.mp4', duration_us: 1000000, width: 1920, height: 1080, state: 'ready' },
    { id: 'm2', name: 'after.mp4', duration_us: 1000000, width: 1920, height: 1080, state: 'ready' },
  ])
  videoApi.activeRecording.mockResolvedValue(null)
  videoApi.mediaFrames.mockResolvedValue({ frame_count: 2, first_pts_us: 0, last_pts_us: 40000, current: { index: 1, pts_us: 40000 } })
  videoApi.mediaFrameNeighbors.mockResolvedValue({ index: 1, pts_us: 40000, prev: null, next: null })
  fetchStub.mockReset()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('P1-VREF 媒体引用一致性', () => {
  it('以保存成功后的项目集合计算引用，并保留其它 Package/plugin 引用', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { id: 'm2', refs: [] }))
    const wrapper = await openDirtyProject()

    await wrapper.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()

    expect(videoApi.putProject).toHaveBeenCalledWith('pkg.scope', 'p1', expect.any(String), { expectedVersion: 'v1' })
    expect(videoApi.listProjectEntries).toHaveBeenCalledTimes(2)
    expect(fetchStub).toHaveBeenCalledWith('/api/media/m2/refs', expect.objectContaining({ method: 'POST' }))
    const body = JSON.parse(fetchStub.mock.calls[0][1].body)
    expect(body).toEqual({ refs: [
      { package_id: 'other.pkg', plugin_id: 'other.plugin', kind: 'workspace' },
      { package_id: 'pkg.scope', plugin_id: 'gamer.video', kind: 'project' },
    ] })
    expect(wrapper.find('[data-testid="project-dirty"]').text()).toContain('已保存')
    expect(wrapper.find('[data-testid="project-save-status"]').text()).toContain('媒体引用已同步')
    expect(wrapper.find('[data-testid="media-ref-sync-error"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('引用同步失败不回滚项目保存，独立重试不再次 PUT 项目', async () => {
    fetchStub
      .mockRejectedValueOnce(new TypeError('refs endpoint unavailable'))
      .mockResolvedValue(jsonResponse(200, { id: 'm2', refs: [] }))
    const wrapper = await openDirtyProject()

    await wrapper.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="project-save-status"]').text()).toContain('项目已保存')
    expect(wrapper.find('[data-testid="project-save-status"]').text()).toContain('媒体引用同步失败')
    expect(wrapper.find('[data-testid="media-ref-sync-error"]').text()).toContain('可直接重试')
    expect(wrapper.find('[data-testid="project-dirty"]').text()).toContain('已保存')
    expect(videoApi.putProject).toHaveBeenCalledTimes(1)

    await wrapper.find('[data-testid="media-ref-sync-retry"]').trigger('click')
    await flushPromises()

    expect(videoApi.putProject).toHaveBeenCalledTimes(1)
    expect(wrapper.find('[data-testid="media-ref-sync-error"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="project-save-status"]').text()).toContain('媒体引用已同步')
    expect(fetchStub).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('项目 PUT 失败与引用同步失败分离：保存失败不发 refs 请求', async () => {
    const wrapper = await openDirtyProject()
    videoApi.putProject.mockRejectedValueOnce(new Error('project resource unavailable'))

    await wrapper.find('[data-testid="project-save"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="project-conflict-banner"]').text()).toContain('项目保存失败')
    expect(wrapper.find('[data-testid="project-save-status"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="media-ref-sync-error"]').exists()).toBe(false)
    expect(fetchStub).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('setMediaRefs 接受与 Workbench 一致的 snake_case 引用字段并正确编码 media id', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { id: 'm 2', refs: [] }))
    await videoApi.setMediaRefs('m 2', [{ package_id: 'pkg.scope', plugin_id: 'gamer.video', kind: 'project' }])

    expect(fetchStub).toHaveBeenCalledWith('/api/media/m%202/refs', expect.objectContaining({ method: 'POST' }))
    expect(JSON.parse(fetchStub.mock.calls[0][1].body)).toEqual({ refs: [
      { package_id: 'pkg.scope', plugin_id: 'gamer.video', kind: 'project' },
    ] })
  })
})
