// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import VideoProjects from './components/video/VideoProjects.vue'
import {
  assetIdentityStatus,
  newProject,
  relinkProjectAsset,
  replaceProjectAsset,
  validateProject,
  withMarker,
  withPrimaryProjectAsset,
  withProjectAsset,
  withoutProjectAsset,
} from './components/video/videoProject'

const HASH_A = 'a'.repeat(64)
const HASH_B = 'b'.repeat(64)
const MEDIA_A = { id: 'media-a', name: '登录录制', sha256: HASH_A, duration_us: 1000000, width: 1920, height: 1080 }
const MEDIA_B = { id: 'media-b', name: '战斗录制', sha256: HASH_B, duration_us: 2000000, width: 1280, height: 720 }

function baseProject() {
  return newProject({ id: 'project-a', name: '显示名称', packageId: 'pkg-a', media: MEDIA_A })
}

function projectWithProductionData() {
  const project = withProjectAsset(baseProject(), MEDIA_B)
  const marked = withMarker(project, {
    label: '点击',
    frame: { media_id: MEDIA_A.id, frame_index: 3, pts_us: 90000, calibration_version: 1 },
  })
  return { ...marked, recording: { recording_id: 'recording-a' } }
}

describe('Video Project 素材操作模型', () => {
  it('添加附加素材只保存 media_id 与快照，不把显示名混入资源 schema', () => {
    const project = withProjectAsset(baseProject(), MEDIA_B)
    expect(project.assets).toHaveLength(2)
    expect(project.assets[1]).toEqual({
      media_id: MEDIA_B.id,
      role: 'reference',
      sha256: HASH_B,
      duration_us: 2000000,
      frame_count: null,
    })
    expect(project.assets[1].name).toBeUndefined()
    expect(validateProject(project)).toEqual([])
  })

  it('不允许直接移除主素材，附加素材可移除', () => {
    const project = withProjectAsset(baseProject(), MEDIA_B)
    expect(() => withoutProjectAsset(project, MEDIA_A.id)).toThrowError(/不能直接移除主素材/)
    const next = withoutProjectAsset(project, MEDIA_B.id)
    expect(next.assets.map(asset => asset.media_id)).toEqual([MEDIA_A.id])
  })

  it('设置附加素材为主素材会清除旧制作含义并重置新视频校准', () => {
    const project = projectWithProductionData()
    const next = withPrimaryProjectAsset(project, MEDIA_B.id, { media: MEDIA_B })
    expect(next.assets.map(asset => [asset.media_id, asset.role])).toEqual([
      [MEDIA_A.id, 'reference'],
      [MEDIA_B.id, 'primary'],
    ])
    expect(next.markers).toEqual([])
    expect(next.recording).toBeNull()
    expect(next.progress.stage).toBe('needs_validation')
    expect(next.calibration.reference_size).toEqual({ width: 1280, height: 720 })
  })

  it('替换不同视频不会静默保留标记、校准或录制事件', () => {
    const project = projectWithProductionData()
    const replacement = { ...MEDIA_B, id: 'media-c', name: '另一段视频', sha256: 'c'.repeat(64) }
    const next = replaceProjectAsset(project, MEDIA_A.id, replacement)
    expect(next.assets[0]).toMatchObject({ media_id: 'media-c', role: 'primary', sha256: 'c'.repeat(64) })
    expect(next.markers).toEqual([])
    expect(next.recording).toBeNull()
    expect(next.progress.stage).toBe('needs_validation')
    expect(next.calibration.reference_size).toEqual({ width: 1280, height: 720 })
  })

  it('重关联只接受 sha256 明确匹配，并同步改写帧身份的 media_id', () => {
    const project = baseProject()
    const missing = { ...project, assets: [{ ...project.assets[0], media_id: 'missing-media' }] }
    const marked = withMarker(missing, {
      label: '旧帧',
      frame: { media_id: 'missing-media', frame_index: 2, pts_us: 60000, calibration_version: 1 },
    })
    const recovered = relinkProjectAsset(marked, 'missing-media', { ...MEDIA_A, id: 'restored-media' })
    expect(recovered.assets[0].media_id).toBe('restored-media')
    expect(recovered.markers[0].frame.media_id).toBe('restored-media')
    expect(assetIdentityStatus(marked.assets[0], MEDIA_A)).toBe('match')
    expect(() => relinkProjectAsset(marked, 'missing-media', MEDIA_B)).toThrowError(/sha256/)
    expect(() => relinkProjectAsset(marked, 'missing-media', { ...MEDIA_A, sha256: '' })).toThrowError(/sha256/)
  })
})

describe('VideoProjects 素材操作面板', () => {
  it('显示名与资源 ID 分开编辑，并显示素材名与 media_id', async () => {
    const wrapper = mount(VideoProjects, {
      props: {
        projects: [{ id: 'project-a', name: '显示名称', valid: true, markerCount: 0, assetCount: 2 }],
        project: projectWithProductionData(),
        mediaList: [MEDIA_A, MEDIA_B],
        selectedMediaId: MEDIA_B.id,
      },
    })
    expect(wrapper.find('[data-testid="project-asset-row"]').text()).toContain('登录录制')
    expect(wrapper.find('[data-testid="project-asset-row"]').text()).toContain(MEDIA_A.id)

    await wrapper.find('[data-testid="project-rename-name"]').trigger('click')
    await wrapper.find('[data-testid="project-rename-input"]').setValue('新显示名称')
    await wrapper.find('[data-testid="project-rename-confirm"]').trigger('click')
    expect(wrapper.emitted('rename-name')[0][0]).toEqual({ id: 'project-a', name: '新显示名称' })

    await wrapper.find('[data-testid="project-rename"]').trigger('click')
    await wrapper.find('[data-testid="project-rename-input"]').setValue('project-renamed')
    await wrapper.find('[data-testid="project-rename-confirm"]').trigger('click')
    expect(wrapper.emitted('rename')[0]).toEqual(['project-a', 'project-renamed'])
    wrapper.unmount()
  })

  it('添加附加素材、切换主素材和明确重关联都只上抛项目副本', async () => {
    const wrapper = mount(VideoProjects, {
      props: {
        project: baseProject(),
        mediaList: [MEDIA_A, MEDIA_B],
        selectedMediaId: MEDIA_B.id,
      },
    })
    await wrapper.find('[data-testid="asset-add"]').trigger('click')
    const added = wrapper.emitted('asset-add')[0][0]
    expect(added.project.assets.map(asset => asset.media_id)).toEqual([MEDIA_A.id, MEDIA_B.id])
    expect(added.project.assets[1].role).toBe('reference')

    await wrapper.find('[data-testid="asset-set-primary"]').trigger('click')
    const primary = wrapper.emitted('asset-primary')[0][0]
    expect(primary.project.assets.find(asset => asset.media_id === MEDIA_B.id).role).toBe('primary')
    expect(primary.project.progress.stage).toBe('needs_validation')
    wrapper.unmount()

    const missing = { ...baseProject(), assets: [{ ...baseProject().assets[0], media_id: 'missing-media' }] }
    const relinkWrapper = mount(VideoProjects, {
      props: { project: missing, mediaList: [{ ...MEDIA_A, id: 'restored-media' }], selectedMediaId: 'restored-media' },
    })
    await relinkWrapper.find('[data-testid="asset-relink"]').trigger('click')
    expect(relinkWrapper.emitted('asset-relink')[0][0].project.assets[0].media_id).toBe('restored-media')
    relinkWrapper.unmount()
  })
})
