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
/** 素材引用上限（与服务端 MAX_ASSETS 同值）。 */
export const MAX_ASSETS = 16

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
  if (assets.length > MAX_ASSETS) out.push({ code: 'asset.too_many', message: `素材引用超过上限 ${MAX_ASSETS}` })
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
 * 媒体库条目转为项目允许保存的快照字段。
 *
 * 媒体显示名只属于媒体库 UI，不写进 Video Project schema；项目资源 id
 * 也不借用显示名。这样重命名媒体不会悄悄改变项目引用身份。
 */
export function projectAssetSnapshot(media, role = 'reference') {
  const mediaId = String(media?.id || '').trim()
  if (!mediaId) {
    throw projectError([{ code: 'asset.media_id', message: '素材 media_id 不能为空' }])
  }
  const duration = Number(media?.duration_us)
  const frameCount = Number(media?.frame_count)
  return {
    media_id: mediaId,
    role: role === 'primary' ? 'primary' : 'reference',
    sha256: String(media?.sha256 || ''),
    duration_us: Number.isFinite(duration) && media?.duration_us !== null
      ? Math.max(0, Math.round(duration))
      : null,
    frame_count: Number.isFinite(frameCount) && media?.frame_count !== null
      ? Math.max(0, Math.round(frameCount))
      : null,
  }
}

function assetIndex(project, mediaId) {
  return (Array.isArray(project?.assets) ? project.assets : [])
    .findIndex(asset => String(asset?.media_id || '') === String(mediaId || ''))
}

function assertAssetChange(project, mediaId, operation) {
  const index = assetIndex(project, mediaId)
  if (index < 0) {
    throw projectError([{
      code: 'asset.not_found',
      message: `项目素材不存在，无法${operation}: ${mediaId}`,
      path: 'assets',
    }])
  }
  return index
}

function cloneProject(project) {
  return JSON.parse(JSON.stringify(project))
}

function mediaDimensions(media) {
  const width = Math.max(1, Math.round(Number(media?.width) || 1))
  const height = Math.max(1, Math.round(Number(media?.height) || 1))
  return { width, height }
}

function invalidatedAfterMediaChange(project, assets, { primaryChanged, media } = {}) {
  const next = {
    ...cloneProject(project),
    assets,
    // Do not carry frame/event meaning across a different video. The user can
    // deliberately rebuild these artifacts after the replacement.
    markers: [],
    recording: null,
    progress: { stage: 'needs_validation', updated_at: nowIso() },
  }
  if (primaryChanged && media) {
    next.calibration = identityCalibration(orientedSize(mediaDimensions(media), 0))
  }
  return touch(next)
}

/**
 * 添加附加素材（V1 role=reference）。只产生本地项目副本，由宿主复用既有
 * expected_version PUT + media refs 安全保存路径；本函数不触碰网络。
 */
export function withProjectAsset(project, media) {
  const assets = Array.isArray(project?.assets) ? project.assets : []
  const mediaId = String(media?.id || '').trim()
  if (!mediaId) throw projectError([{ code: 'asset.media_id', message: '素材 media_id 不能为空' }])
  if (assetIndex(project, mediaId) >= 0) {
    throw projectError([{ code: 'asset.duplicate', message: `素材已在项目中: ${mediaId}`, path: 'assets' }])
  }
  if (assets.length >= MAX_ASSETS) {
    throw projectError([{ code: 'asset.too_many', message: `素材引用超过上限 ${MAX_ASSETS}`, path: 'assets' }])
  }
  return touch({
    ...cloneProject(project),
    assets: [...assets.map(asset => ({ ...asset })), projectAssetSnapshot(media, 'reference')],
    progress: { stage: 'assets', updated_at: nowIso() },
  })
}

/** 删除附加素材；主素材必须先设置其它主素材或执行替换，不能制造非法项目。 */
export function withoutProjectAsset(project, mediaId) {
  const index = assertAssetChange(project, mediaId, '移除素材')
  const asset = project.assets[index]
  if (asset.role === 'primary') {
    throw projectError([{
      code: 'asset.primary_required',
      message: '不能直接移除主素材，请先设置其它素材为主素材或替换主素材',
      path: `assets[${index}]`,
    }])
  }
  return touch({
    ...cloneProject(project),
    assets: project.assets.filter((_, assetIndexValue) => assetIndexValue !== index),
    progress: { stage: 'assets', updated_at: nowIso() },
  })
}

/**
 * 设置已有项目素材为主素材。切换到不同视频会清空标记/录制事件并进入
 * needs_validation；有媒体元数据时同时把校准重置为新视频的恒等校准。
 */
export function withPrimaryProjectAsset(project, mediaId, { media } = {}) {
  const index = assertAssetChange(project, mediaId, '设置主素材')
  const target = project.assets[index]
  const current = project.assets.find(asset => asset.role === 'primary')
  if (target.role === 'primary') return cloneProject(project)
  const assets = project.assets.map(asset => ({
    ...asset,
    role: asset.media_id === mediaId ? 'primary' : (asset.role === 'primary' ? 'reference' : asset.role),
  }))
  return invalidatedAfterMediaChange(project, assets, {
    primaryChanged: current?.media_id !== target.media_id,
    media,
  })
}

/** 媒体快照身份检查：只有两侧都有 sha256 且完全一致才算可安全重关联。 */
export function assetIdentityStatus(asset, media) {
  const oldSha = String(asset?.sha256 || '').trim().toLowerCase()
  const newSha = String(media?.sha256 || '').trim().toLowerCase()
  if (oldSha && newSha) return oldSha === newSha ? 'match' : 'mismatch'
  return 'unknown'
}

/**
 * 明确重关联缺失素材。它与 replaceProjectAsset 有意分开：未知或不匹配的
 * sha256 一律拒绝，避免把另一段视频静默当成原素材；换视频必须走 replace。
 * 成功重关联会同步改写标记中的 media_id，但不改变帧/校准/事件含义。
 */
export function relinkProjectAsset(project, missingMediaId, media) {
  const index = assertAssetChange(project, missingMediaId, '重新关联素材')
  const oldAsset = project.assets[index]
  if (assetIdentityStatus(oldAsset, media) !== 'match') {
    const status = assetIdentityStatus(oldAsset, media)
    throw projectError([{
      code: status === 'mismatch' ? 'asset.identity_mismatch' : 'asset.identity_unknown',
      message: status === 'mismatch'
        ? '新素材 sha256 与缺失素材快照不一致，请使用“替换素材”并重新验证制作信息'
        : '缺少可验证的 sha256，不能确认这是同一素材；请使用“替换素材”并重新验证制作信息',
      path: `assets[${index}]`,
    }])
  }
  const nextMediaId = String(media?.id || '').trim()
  if (!nextMediaId) throw projectError([{ code: 'asset.media_id', message: '重关联目标素材 id 不能为空' }])
  if (nextMediaId !== oldAsset.media_id && assetIndex(project, nextMediaId) >= 0) {
    throw projectError([{ code: 'asset.duplicate', message: `素材已在项目中: ${nextMediaId}`, path: 'assets' }])
  }
  const assets = project.assets.map((asset, assetIndexValue) => (
    assetIndexValue === index ? projectAssetSnapshot(media, asset.role) : { ...asset }
  ))
  const next = {
    ...cloneProject(project),
    assets,
    markers: (project.markers || []).map(marker => ({
      ...marker,
      frame: marker.frame?.media_id === oldAsset.media_id
        ? { ...marker.frame, media_id: nextMediaId }
        : { ...marker.frame },
    })),
    progress: { stage: 'assets', updated_at: nowIso() },
  }
  return touch(next)
}

/**
 * 替换项目素材。无论替换主素材还是被标记引用的附加素材，都清除可能带有
 * 旧视频语义的标记与录制事件；主素材替换还重置校准。不会静默保留旧含义。
 */
export function replaceProjectAsset(project, mediaId, media) {
  const index = assertAssetChange(project, mediaId, '替换素材')
  const oldAsset = project.assets[index]
  const nextMediaId = String(media?.id || '').trim()
  if (!nextMediaId) throw projectError([{ code: 'asset.media_id', message: '替换目标素材 id 不能为空' }])
  if (nextMediaId === oldAsset.media_id) {
    return touch({
      ...cloneProject(project),
      assets: project.assets.map((asset, assetIndexValue) => (
        assetIndexValue === index ? projectAssetSnapshot(media, asset.role) : { ...asset }
      )),
    })
  }
  if (assetIndex(project, nextMediaId) >= 0) {
    throw projectError([{ code: 'asset.duplicate', message: `素材已在项目中: ${nextMediaId}`, path: 'assets' }])
  }
  const assets = project.assets.map((asset, assetIndexValue) => (
    assetIndexValue === index ? projectAssetSnapshot(media, asset.role) : { ...asset }
  ))
  return invalidatedAfterMediaChange(project, assets, {
    primaryChanged: oldAsset.role === 'primary',
    media,
  })
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
