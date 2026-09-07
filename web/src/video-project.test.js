/**
 * Phase 6 视频制作项目测试（纯模型层，node 环境）：
 * - calibration.js：四空间坐标变换（encoded→oriented→content→reference），
 *   等比缩放 + letterbox（绝不非等比拉伸）、旋转/像素比例、往返一致性、诊断；
 * - recordingEvents.js：会话时间轴 → 媒体 PTS 的 base_pts_us 整数映射、
 *   分段查找、未对齐诊断、来源标注；
 * - videoProject.js：schema v1 解析/校验（与服务端 project.rs 规则镜像）、
 *   标记 CRUD（帧身份）、校准版本递增与标脏、素材缺失状态；
 * - videoApi 项目端点：URL/body 形态逐字锁定（Package 资源三元组）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  calibrationDiagnostics,
  contentRectOrDefault,
  contentToReferenceTransform,
  encodedToOriented,
  encodedToReference,
  identityCalibration,
  orientedCanvas,
  orientedSize,
  orientedToEncoded,
  referenceToEncoded,
} from './components/video/calibration'
import {
  alignEvents,
  eventMediaPosition,
  eventSourceLabel,
  segmentForTimeline,
  sortEvents,
} from './components/video/recordingEvents'
import {
  assetStatus,
  isMarkerStale,
  isValidProjectId,
  newProject,
  parseProject,
  projectFilePath,
  projectIdFromPath,
  serializeProject,
  validateProject,
  withCalibration,
  withMarker,
  withMarkerText,
  withoutMarker,
} from './components/video/videoProject'
import { videoApi } from './components/video/videoApi'

const MEDIA = { id: 'm1', name: '录制.mp4', sha256: 'a'.repeat(64), duration_us: 6_500_000, width: 1920, height: 1080 }

// ---------------------------------------------------------------------------
// calibration.js
// ---------------------------------------------------------------------------

describe('calibration 坐标变换', () => {
  it('恒等校准：encoded == reference，点变换为恒等', () => {
    const calibration = identityCalibration({ width: 1920, height: 1080 })
    expect(calibration.version).toBe(1)
    expect(calibration.reference_size).toEqual({ width: 1920, height: 1080 })
    const point = encodedToReference({ x: 500, y: 300 }, calibration, { width: 1920, height: 1080 })
    expect(point.x).toBeCloseTo(500, 6)
    expect(point.y).toBeCloseTo(300, 6)
  })

  it('旋转 90°：oriented 宽高互换；变换往返一致（round-trip）', () => {
    const encoded = { width: 1920, height: 1080 }
    expect(orientedSize(encoded, 90)).toEqual({ width: 1080, height: 1920 })
    const calibration = { ...identityCalibration(orientedSize(encoded, 90)), rotation: 90 }
    const p = { x: 100, y: 700 }
    const back = orientedToEncoded(encodedToOriented(p, calibration, encoded), calibration, encoded)
    expect(back.x).toBeCloseTo(p.x, 6)
    expect(back.y).toBeCloseTo(p.y, 6)
    // reference 空间同样可往返
    const ref = encodedToReference(p, calibration, encoded)
    const back2 = referenceToEncoded(ref, calibration, encoded)
    expect(back2.x).toBeCloseTo(p.x, 6)
    expect(back2.y).toBeCloseTo(p.y, 6)
  })

  it('黑边裁剪 + 参考分辨率：等比缩放，纵横比不一致时居中补边（不拉伸）', () => {
    const encoded = { width: 1920, height: 1080 }
    // 内容区 1920×800（上下黑边），参考 1920×800 → scale = 1
    const calibration = {
      version: 2,
      rotation: 0,
      pixel_aspect: { num: 1, den: 1 },
      content_rect: { x: 0, y: 140, w: 1920, h: 800 },
      reference_size: { width: 1920, height: 800 },
    }
    expect(contentRectOrDefault(calibration, encoded)).toEqual({ x: 0, y: 140, w: 1920, h: 800 })
    const transform = contentToReferenceTransform(calibration, encoded)
    // 单一缩放因子：x/y 同比例（结构性排除非等比拉伸）
    expect(transform.scale).toBeCloseTo(1, 9)
    // 内容区内点 1:1 映射；区域外的 y 被裁剪基准平移（140 → 0）
    const point = encodedToReference({ x: 960, y: 540 }, calibration, encoded)
    expect(point.x).toBeCloseTo(960, 6)
    expect(point.y).toBeCloseTo(400, 6)
  })

  it('letterbox：内容区比例宽于参考 → 按 min 缩放 + 水平居中补边', () => {
    const encoded = { width: 2000, height: 1000 }
    const calibration = {
      version: 1,
      rotation: 0,
      pixel_aspect: { num: 1, den: 1 },
      content_rect: { x: 0, y: 0, w: 2000, h: 1000 },
      reference_size: { width: 1000, height: 600 },
    }
    const transform = contentToReferenceTransform(calibration, encoded)
    // min(1000/2000, 600/1000) = 0.5；水平补边 (1000-2000*0.5)/2 = 0，垂直 (600-1000*0.5)/2 = 50
    expect(transform.scale).toBeCloseTo(0.5, 9)
    expect(transform.padX).toBeCloseTo(0, 9)
    expect(transform.padY).toBeCloseTo(50, 9)
    // 内容中心 → 参考中心（等比 + 居中），且两轴缩放一致
    const center = encodedToReference({ x: 1000, y: 500 }, calibration, encoded)
    expect(center.x).toBeCloseTo(500, 6)
    expect(center.y).toBeCloseTo(300, 6)
    const square = encodedToReference({ x: 1000, y: 0 }, calibration, encoded)
    expect(square.x).toBeCloseTo(500, 6)
    expect(square.y).toBeCloseTo(50, 6)
  })

  it('像素比例 2:1：oriented 画布宽度翻倍（显示侧校正，非破坏）', () => {
    const encoded = { width: 640, height: 480 }
    const calibration = { ...identityCalibration({ width: 640, height: 480 }), pixel_aspect: { num: 2, den: 1 } }
    expect(orientedCanvas(encoded, calibration).width).toBeCloseTo(1280, 6)
    const back = orientedToEncoded(encodedToOriented({ x: 640, y: 120 }, calibration, encoded), calibration, encoded)
    expect(back.x).toBeCloseTo(640, 6)
    expect(back.y).toBeCloseTo(120, 6)
  })

  it('校准诊断：非法值逐条报告；合法校准零诊断', () => {
    const diagnostics = calibrationDiagnostics({
      version: 0, rotation: 45, pixel_aspect: { num: 0, den: 1 },
      content_rect: { x: -1, y: 0, w: 0, h: 10 }, reference_size: { width: 0, height: 10 },
    })
    const codes = diagnostics.map(d => d.code)
    for (const code of ['calibration.version', 'calibration.rotation', 'calibration.pixel_aspect', 'calibration.content_rect', 'calibration.reference_size']) {
      expect(codes).toContain(code)
    }
    expect(calibrationDiagnostics(identityCalibration({ width: 10, height: 10 }))).toEqual([])
  })
})

// ---------------------------------------------------------------------------
// recordingEvents.js（base_pts_us 整数映射）
// ---------------------------------------------------------------------------

const SEGMENTS = [
  { media_id: 'seg-a', start_us: 0, duration_us: 5_000_000, base_pts_us: 1_000, reason: 'normal' },
  { media_id: 'seg-b', start_us: 5_000_000, duration_us: 4_000_000, base_pts_us: 60_000, reason: 'codec_change' },
]

describe('recordingEvents 事件对齐', () => {
  it('segmentForTimeline：按 [start, start+duration) 找段；间隙返回 null', () => {
    expect(segmentForTimeline(SEGMENTS, 0)?.media_id).toBe('seg-a')
    expect(segmentForTimeline(SEGMENTS, 4_999_999)?.media_id).toBe('seg-a')
    expect(segmentForTimeline(SEGMENTS, 5_000_000)?.media_id).toBe('seg-b')
    // 5s 段间隙之外越界
    expect(segmentForTimeline(SEGMENTS, 9_000_001)).toBeNull()
    expect(segmentForTimeline([], 0)).toBeNull()
  })

  it('eventMediaPosition：media_pts = base_pts_us + timeline_us − start_us（整数微秒）', () => {
    const position = eventMediaPosition({ timeline_us: 1_200_000 }, SEGMENTS)
    expect(position.media_id).toBe('seg-a')
    expect(position.pts_us).toBe(1_000 + 1_200_000)
    const inSecondSegment = eventMediaPosition({ timeline_us: 6_000_000 }, SEGMENTS)
    expect(inSecondSegment.media_id).toBe('seg-b')
    expect(inSecondSegment.pts_us).toBe(60_000 + 1_000_000)
    expect(eventMediaPosition({ timeline_us: 99_000_000 }, SEGMENTS)).toBeNull()
  })

  it('alignEvents：升序 + 来源标注 + 未对齐诊断；wait/text 摘要可读', () => {
    const views = alignEvents([
      { event_id: 'e2', source: 'runner', kind: 'wait', status: 'accepted', timeline_us: 6_000_000, payload: { duration_us: 800_000 } },
      { event_id: 'e1', source: 'manual', kind: 'tap', status: 'accepted', timeline_us: 1_200_000, payload: { x: 820, y: 460 } },
      { event_id: 'e3', source: 'manual', kind: 'tap', status: 'accepted', timeline_us: 99_000_000, payload: {} },
    ], SEGMENTS)
    expect(views.map(view => view.event.event_id)).toEqual(['e1', 'e2', 'e3'])
    expect(views[0].sourceLabel).toBe('手动')
    expect(views[1].sourceLabel).toBe('脚本')
    expect(views[0].ptsUs).toBe(1_201_000)
    expect(views[2].unmapped).toBe(true)
    expect(views[2].ptsUs).toBeNull()
    expect(eventSourceLabel('keymap')).toBe('键映射')
    // 防御性排序
    expect(sortEvents([{ timeline_us: 9 }, { timeline_us: 1 }])[0].timeline_us).toBe(1)
  })
})

// ---------------------------------------------------------------------------
// videoProject.js（schema v1 镜像校验 + 编辑操作）
// ---------------------------------------------------------------------------

function baseProject() {
  const project = newProject({ id: 'daily-login', name: '每日登录', packageId: 'hkrpg', media: MEDIA })
  return project
}

describe('videoProject 模型', () => {
  it('新建项目：primary 素材快照 + 恒等校准 + v1 schema', () => {
    const project = baseProject()
    expect(project.schema_version).toBe(1)
    expect(project.assets).toHaveLength(1)
    expect(project.assets[0]).toMatchObject({ media_id: 'm1', role: 'primary', sha256: 'a'.repeat(64) })
    expect(project.calibration.reference_size).toEqual({ width: 1920, height: 1080 })
    expect(project.markers).toEqual([])
    expect(validateProject(project)).toEqual([])
  })

  it('解析/序列化往返；坏数据诊断与服务端镜像（版本/引用/帧身份）', () => {
    const project = baseProject()
    const parsed = parseProject(serializeProject(project))
    expect(parsed.id).toBe('daily-login')

    expect(() => parseProject('{oops')).toThrowError()
    const corrupt = { ...project, schema_version: 9 }
    expect(() => parseProject(serializeProject(corrupt))).toThrowError()

    const noAsset = { ...project, assets: [] }
    expect(validateProject(noAsset).map(d => d.code)).toContain('asset.required')
    const twoPrimary = { ...project, assets: [project.assets[0], { ...project.assets[0], media_id: 'm2' }] }
    expect(validateProject(twoPrimary).map(d => d.code)).toContain('asset.primary_count')
    const badSha = { ...project, assets: [{ ...project.assets[0], sha256: 'xyz' }] }
    expect(validateProject(badSha).map(d => d.code)).toContain('asset.sha256')
    const badMarker = { ...project, markers: [{ id: 'mk-1', label: '', note: '', created_at: '', frame: { media_id: 'ghost', frame_index: -1, pts_us: 0, calibration_version: 0 } }] }
    const codes = validateProject(badMarker).map(d => d.code)
    expect(codes).toContain('marker.media_not_found')
    expect(codes).toContain('marker.frame')
    expect(codes).toContain('marker.calibration_version')
    expect(validateProject({ ...project, markers: [] }).filter(d => d.code.startsWith('marker'))).toEqual([])
  })

  it('项目 id 语法与路径换算（与 server validate_scope_id 同规则）', () => {
    expect(isValidProjectId('daily-login')).toBe(true)
    expect(isValidProjectId('a.b_c-9')).toBe(true)
    expect(isValidProjectId('Bad')).toBe(false)
    expect(isValidProjectId('-x')).toBe(false)
    expect(isValidProjectId('')).toBe(false)
    expect(projectFilePath('p1')).toBe('projects/p1.json')
    expect(projectIdFromPath('projects/p1.json')).toBe('p1')
    expect(projectIdFromPath('other/p1.json')).toBeNull()
  })

  it('标记 CRUD：帧身份引用（非浮点秒）；注释更新；删除', () => {
    let project = baseProject()
    project = withMarker(project, {
      label: '开始点击',
      frame: { media_id: 'm1', frame_index: 12, pts_us: 333_000, calibration_version: 1 },
    })
    expect(project.markers[0]).toMatchObject({
      id: 'mk-1',
      frame: { media_id: 'm1', frame_index: 12, pts_us: 333_000, calibration_version: 1 },
    })
    project = withMarkerText(project, 'mk-1', { note: '这里要等动画结束' })
    expect(project.markers[0].note).toContain('等动画结束')
    project = withoutMarker(project, 'mk-1')
    expect(project.markers).toHaveLength(0)
  })

  it('校准版本：值变化 → version 递增；等值应用不虚增；旧校准标记标脏', () => {
    let project = baseProject()
    project = withMarker(project, { label: 'm', frame: { media_id: 'm1', frame_index: 1, pts_us: 10, calibration_version: 1 } })
    expect(isMarkerStale(project, project.markers[0])).toBe(false)

    const next = withCalibration(project, { ...project.calibration, rotation: 90 })
    expect(next.calibration.version).toBe(2)
    expect(isMarkerStale(next, next.markers[0])).toBe(true)
    expect(next.progress.stage).toBe('calibration')

    const same = withCalibration(next, { ...next.calibration, rotation: 90 })
    expect(same.calibration.version).toBe(2)
  })

  it('素材缺失状态：项目可打开、缺失可诊断（引用不复制）', () => {
    const project = baseProject()
    expect(assetStatus(project, [MEDIA]).ready).toBe(true)
    const missing = assetStatus(project, [])
    expect(missing.ready).toBe(false)
    expect(missing.primary).toBeNull()
    expect(missing.missingAssets).toEqual(['m1'])
  })
})

// ---------------------------------------------------------------------------
// videoApi 项目端点（Package 资源三元组）
// ---------------------------------------------------------------------------

function jsonResponse(status, body, contentType = 'application/json') {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => (String(name).toLowerCase() === 'content-type' ? contentType : null) },
    json: async () => body,
  }
}

describe('videoApi 项目端点（Package 资源 API）', () => {
  let fetchStub
  beforeEach(() => {
    fetchStub = vi.fn()
    vi.stubGlobal('fetch', fetchStub)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('listProjectEntries → GET resources?prefix=projects/，解包 resources 数组', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { resources: [{ path: 'projects/p1.json', content: '{}' }] }))
    await expect(videoApi.listProjectEntries('my pkg')).resolves.toHaveLength(1)
    expect(fetchStub.mock.calls[0][0]).toBe(`/api/packages/my%20pkg/plugins/gamer.video/resources?prefix=${encodeURIComponent('projects/')}`)
  })

  it('getProject / putProject / deleteProject / projectUrl 走三元组路径（%2F 安全分段编码）', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { path: 'projects/p1.json', content: '{}', version: 'abc123' }))
    const entry = await videoApi.getProject('pkg', 'p1')
    expect(entry.version).toBe('abc123')
    expect(fetchStub.mock.calls[0][0]).toBe('/api/packages/pkg/plugins/gamer.video/resources/projects/p1.json')

    fetchStub.mockResolvedValue(jsonResponse(200, { version: 'def456', ok: true }))
    const saved = await videoApi.putProject('pkg', 'p1', '{"k":1}', { expectedVersion: 'abc123' })
    expect(saved.version).toBe('def456')
    const [url, options] = fetchStub.mock.calls[1]
    expect(url).toBe('/api/packages/pkg/plugins/gamer.video/resources/projects/p1.json')
    expect(options.method).toBe('PUT')
    expect(JSON.parse(options.body)).toEqual({ content: '{"k":1}', expected_version: 'abc123' })

    // force: false 不序列化 force 字段
    await videoApi.putProject('pkg', 'p1', 'x', { expectedVersion: 'v', force: false })
    expect(JSON.parse(fetchStub.mock.calls[2][1].body).force).toBeUndefined()

    fetchStub.mockResolvedValue(jsonResponse(204, null))
    await expect(videoApi.deleteProject('pkg', 'p1')).resolves.toBeNull()
    expect(fetchStub.mock.calls[3][0]).toBe('/api/packages/pkg/plugins/gamer.video/resources/projects/p1.json')
    // URL 构造器不发请求
    expect(videoApi.projectUrl('pkg', 'p.1_x')).toBe('/api/packages/pkg/plugins/gamer.video/resources/projects/p.1_x.json')
  })

  it('putProject 保存冲突（409 invalid_content/version_conflict）原样上抛 status/code', async () => {
    fetchStub.mockResolvedValue(jsonResponse(409, { error: 'version_conflict: 资源已被其他页面修改' }))
    await expect(videoApi.putProject('pkg', 'p1', 'x', { expectedVersion: 'old' })).rejects.toMatchObject({
      status: 409,
      code: 'version_conflict: 资源已被其他页面修改',
    })
  })

  it('renameProject → POST rename，body {path, new_path}', async () => {
    fetchStub.mockResolvedValue(jsonResponse(200, { ok: true, path: 'projects/p2.json' }))
    await videoApi.renameProject('pkg', 'p1', 'p2')
    const [url, options] = fetchStub.mock.calls[0]
    expect(url).toBe('/api/packages/pkg/plugins/gamer.video/rename')
    expect(options.method).toBe('POST')
    expect(JSON.parse(options.body)).toEqual({ path: 'projects/p1.json', new_path: 'projects/p2.json' })
  })
})
