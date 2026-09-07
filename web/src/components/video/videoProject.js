/**
 * Video Project 前端模型（Phase 6，计划 §9.1）。
 *
 * 项目 = Package 资源 `plugins/gamer.video/projects/<project-id>.json`
 * （乐观并发 expected_version；dormant 保留）。schema v1 的权威定义在
 * `server/src/extensions/video/project.rs`——本模块是同规则的**前端镜像**：
 * 保存前本地校验（服务端钩子接线前的第一道闸），字段与诊断码逐字对齐。
 *
 * 核心纪律：
 * - 原视频**不复制**进 Package：assets 只存逻辑引用 + 快照（sha256/时长/帧数），
 *   缺失状态由 `assetStatus()` 对媒体库列表现算（项目可打开、可诊断）。
 * - 标记引用**帧身份**（frame_index + pts_us + calibration_version），
 *   绝不存浏览器浮点秒。
 * - 校准值变化必须递增 `calibration.version`；标记版本落后 = 标脏（stale），
 *   提示重新确认而不是悄悄变形。
 */

import { calibrationDiagnostics, identityCalibration, orientedSize } from './calibration'

/** 当前 schema 版本（唯一受支持；与服务端 PROJECT_SCHEMA_VERSION 锁同值）。 */
export const PROJECT_SCHEMA_VERSION = 1
/** 项目资源目录（相对 plugins/gamer.video/）。 */
export const PROJECT_DIR = 'projects'
/** 标记上限（与服务端 MAX_MARKERS 同值）。 */
export const MAX_MARKERS = 500

const SCOPE_ID_RE = /^[a-z0-9][a-z0-9._-]*$/

/** 项目 id 语法（与 server validate_scope_id 同规则）。 */
export function isValidProjectId(id) {
  return SCOPE_ID_RE.test(String(id || '')) && String(id).length <= 64
}

/** 项目资源路径：`projects/<id>.json`。 */
export function projectFilePath(id) {
  return `${PROJECT_DIR}/${String(id)}.json`
}

/** 从资源路径反解项目 id；非项目路径返回 null。 */
export function projectIdFromPath(path) {
  const match = /^projects\/(.+)\.json$/.exec(String(path || ''))
  return match ? match[1] : null
}

function nowIso() {
  return new Date().toISOString()
}

/**
 * 新建项目（schema v1）。media = 媒体库元数据（id/sha256/duration_us/width/height），
 * 作为 primary 素材引用 + 快照；校准取恒等（参考尺寸 = oriented 尺寸）。
 */
export function newProject({ id, name, packageId, media }) {
  const encoded = { width: Math.max(1, Math.round(Number(media?.width) || 1)), height: Math.max(1, Math.round(Number(media?.height) || 1)) }
  const sha256 = String(media?.sha256 || '')
  return {
    schema_version: PROJECT_SCHEMA_VERSION,
    id: String(id),
    name: String(name || id),
    package_id: String(packageId || ''),
    notes: '',
    created_at: nowIso(),
    updated_at: nowIso(),
    assets: [{
      media_id: String(media?.id || ''),
      role: 'primary',
      sha256,
      duration_us: Number.isFinite(Number(media?.duration_us)) && media?.duration_us !== null
        ? Math.max(0, Math.round(Number(media.duration_us)))
        : null,
      frame_count: null,
    }],
    recording: null,
    calibration: identityCalibration(orientedSize(encoded, 0)),
    markers: [],
    progress: { stage: 'created', updated_at: nowIso() },
  }
}

/** 序列化为存储文本（2 空格缩进 JSON）。 */
export function serializeProject(project) {
  return JSON.stringify(project, null, 2)
}

/**
 * 解析项目文本；失败抛带 `diagnostics` 数组的 Error（结构与服务端诊断一致）。
 */
export function parseProject(text) {
  let project
  try {
    project = JSON.parse(text)
  } catch (error) {
    throw projectError([{ code: 'json.parse', message: `项目 JSON 解析失败: ${error.message}` }])
  }
  const diagnostics = validateProject(project)
  if (diagnostics.length) throw projectError(diagnostics)
  return project
}

function projectError(diagnostics) {
  const error = new Error(diagnostics[0]?.message || '项目数据无效')
  error.name = 'VideoProjectError'
  error.diagnostics = diagnostics
  return error
}

/** 结构校验（前端镜像；不含保存位置上下文）。返回诊断数组，空 = 合法。 */
export function validateProject(project) {
  const out = []
  if (!project || typeof project !== 'object' || Array.isArray(project)) {
    return [{ code: 'json.parse', message: '项目必须是 JSON 对象' }]
  }
  if (project.schema_version !== PROJECT_SCHEMA_VERSION) {
    out.push({ code: 'version.unsupported', message: `项目 schema_version 仅支持 ${PROJECT_SCHEMA_VERSION}（得到 ${project.schema_version}）` })
  }
  if (!isValidProjectId(project.id)) {
    out.push({ code: 'id.invalid', message: '项目 id 必须匹配 [a-z0-9][a-z0-9._-]*（≤64 字符，禁大写）' })
  }
  if (!String(project.name ?? '').trim()) {
    out.push({ code: 'name.required', message: '项目名不能为空' })
  } else if (String(project.name).length > 120) {
    out.push({ code: 'name.too_long', message: '项目名超过 120 字符' })
  }
  // assets
  const assets = Array.isArray(project.assets) ? project.assets : []
  if (!assets.length) out.push({ code: 'asset.required', message: '项目至少引用一个媒体素材' })
  if (assets.length > 16) out.push({ code: 'asset.too_many', message: '素材引用超过上限 16' })
  let primaries = 0
  const assetIds = new Set()
  assets.forEach((asset, index) => {
    const path = `assets[${index}]`
    if (!String(asset?.media_id ?? '').trim()) out.push({ code: 'asset.media_id', message: '素材 media_id 不能为空', path })
    if (asset?.role === 'primary') primaries += 1
    else if (asset?.role !== 'reference') out.push({ code: 'asset.role', message: '素材 role 只能是 primary|reference', path })
    const sha = String(asset?.sha256 ?? '')
    if (sha && !/^[0-9a-fA-F]{64}$/.test(sha)) out.push({ code: 'asset.sha256', message: '素材 sha256 快照必须是 64 位 hex 或留空', path })
    if (asset?.media_id) assetIds.add(String(asset.media_id))
  })
  if (assets.length && primaries !== 1) {
    out.push({ code: 'asset.primary_count', message: `项目必须恰有一个 primary 素材（得到 ${primaries}）` })
  }
  if (project.recording !== null && project.recording !== undefined) {
    if (!String(project.recording?.recording_id ?? '').trim()) {
      out.push({ code: 'recording.invalid', message: '录制会话引用 id 不能为空' })
    }
  }
  // calibration
  calibrationDiagnostics(project.calibration).forEach(diagnostic => {
    out.push({ ...diagnostic, path: `calibration.${diagnostic.code.split('.')[1]}` })
  })
  // markers
  const markers = Array.isArray(project.markers) ? project.markers : []
  if (markers.length > MAX_MARKERS) out.push({ code: 'marker.too_many', message: `标记数超过上限 ${MAX_MARKERS}` })
  const seen = new Set()
  markers.forEach((marker, index) => {
    const path = `markers[${index}]`
    const markerId = String(marker?.id ?? '')
    if (!markerId.trim()) out.push({ code: 'marker.id', message: '标记 id 不能为空', path })
    else if (seen.has(markerId)) out.push({ code: 'marker.duplicate', message: `标记 id 重复: ${markerId}`, path })
    else seen.add(markerId)
    if (String(marker?.label ?? '').length > 120) out.push({ code: 'marker.label', message: '标记名超过 120 字符', path })
    const frame = marker?.frame
    if (!frame || !assetIds.has(String(frame.media_id ?? ''))) {
      out.push({ code: 'marker.media_not_found', message: '标记引用的素材不在项目 assets 内', path })
    }
    if (!Number.isFinite(Number(frame?.frame_index)) || Number(frame?.frame_index) < 0
      || !Number.isFinite(Number(frame?.pts_us)) || Number(frame?.pts_us) < 0) {
      out.push({ code: 'marker.frame', message: '标记帧身份必须是整数 frame_index + pts_us（≥0）', path })
    }
    if (!(Number(frame?.calibration_version) >= 1)) {
      out.push({ code: 'marker.calibration_version', message: '标记帧必须记录有效校准版本（≥1）', path })
    }
  })
  if (project.progress !== null && project.progress !== undefined
    && String(project.progress?.stage ?? '').length > 32) {
    out.push({ code: 'progress.invalid', message: '制作进度 stage 超过 32 字符' })
  }
  return out
}

function touch(project) {
  return { ...project, updated_at: nowIso() }
}

/** 追加标记（纯函数，返回新项目）；帧身份 = 当前锁定帧 + 当前校准版本。 */
export function withMarker(project, { label, note = '', frame }) {
  const n = project.markers.length + 1
  let markerId = `mk-${n}`
  const taken = new Set(project.markers.map(marker => marker.id))
  let suffix = 0
  while (taken.has(markerId)) {
    suffix += 1
    markerId = `mk-${n}-${suffix}`
  }
  return touch({
    ...project,
    markers: [...project.markers, {
      id: markerId,
      label: String(label || `标记 ${n}`).slice(0, 120),
      note: String(note || ''),
      frame: {
        media_id: String(frame.media_id),
        frame_index: Math.max(0, Math.round(Number(frame.frame_index))),
        pts_us: Math.max(0, Math.round(Number(frame.pts_us))),
        calibration_version: Math.max(1, Math.round(Number(frame.calibration_version))),
      },
      created_at: nowIso(),
    }],
    progress: { stage: 'markers', updated_at: nowIso() },
  })
}

/** 删除标记（纯函数）。 */
export function withoutMarker(project, markerId) {
  return touch({ ...project, markers: project.markers.filter(marker => marker.id !== markerId) })
}

/** 更新标记文本（label/note；纯函数）。 */
export function withMarkerText(project, markerId, { label, note }) {
  return touch({
    ...project,
    markers: project.markers.map(marker => (marker.id === markerId
      ? {
        ...marker,
        label: label !== undefined ? String(label).slice(0, 120) : marker.label,
        note: note !== undefined ? String(note) : marker.note,
      }
      : marker)),
  })
}

/** 标记是否基于旧校准（frame.calibration_version ≠ 当前校准版本）。 */
export function isMarkerStale(project, marker) {
  return Number(marker?.frame?.calibration_version) !== Number(project?.calibration?.version)
}

/** 校准更新：值变化时 version 自动递增（相同值不虚增版本）。返回新项目。 */
export function withCalibration(project, calibration) {
  const changed = ['rotation', 'pixel_aspect', 'content_rect', 'reference_size']
    .some(key => JSON.stringify(calibration[key]) !== JSON.stringify(project.calibration[key]))
  if (!changed) return { ...project, calibration: { ...project.calibration } }
  return touch({
    ...project,
    calibration: { ...calibration, version: Number(project.calibration.version) + 1 },
    progress: { stage: 'calibration', updated_at: nowIso() },
  })
}

/** 项目引用的 media id 去重集合（Phase 8 契约：项目保存时用于媒体引用同步）。 */
export function projectMediaIds(project) {
  return [...new Set((Array.isArray(project?.assets) ? project.assets : [])
    .map(asset => String(asset?.media_id || ''))
    .filter(Boolean))]
}

/**
 * 素材引用状态（对媒体库列表现算）：
 * `{primary, ready, missingAssets:[media_id]}`。缺失 ≠ 打不开——项目可打开并
 * 明确标注素材缺失（可诊断缺失状态，计划 §9.1）。
 */
export function assetStatus(project, mediaList) {
  const list = Array.isArray(mediaList) ? mediaList : []
  const ids = new Set(list.map(media => String(media.id)))
  const missingAssets = (project?.assets || [])
    .map(asset => String(asset.media_id))
    .filter(mediaId => !ids.has(mediaId))
  const primaryAsset = (project?.assets || []).find(asset => asset.role === 'primary') || null
  const primary = primaryAsset ? list.find(media => String(media.id) === primaryAsset.media_id) || null : null
  return { primary, ready: !!primary && missingAssets.length === 0, missingAssets }
}
