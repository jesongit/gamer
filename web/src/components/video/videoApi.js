// 视频工作台 REST 封装（实施合同：docs/plans/gamer_video_workbench_contracts.md §1/§2/§6）。
//
// 合同 §4：为解除与 api.js（C 属地）的并行时序耦合，D2 在本目录自建 fetch 直调封装；
// 端点形态逐字对齐合同 §1（媒体）/§2（录制），草稿走既有扩展调用通路
// POST /api/extensions/gamer.yaml/call（action = automation.create_draft，合同 §5）。
// 同源 Cookie 鉴权默认携带（SameSite=Strict，不设 credentials、不引 CSRF token，
// 与 api.js/auth.js 同一口径）；401 交给 auth.js 全站拦截。
// 集成者后续可决定是否把本封装收编进 api.js。

import { handleUnauthorized } from '../../auth'
import { GAMER_VIDEO_PLUGIN_ID, GAMER_YAML_PLUGIN_ID, VIDEO_PROJECT_DIR } from '../../gamer-plugin-ids'

/** 视频工作台 API 的稳定错误形态：调用方按 status / code / data 判断。 */
function makeError(status, code, message, data = null, cause) {
  const error = new Error(message, cause ? { cause } : undefined)
  error.name = 'VideoApiError'
  error.status = status
  error.code = code
  error.data = data
  return error
}

function networkError(cause) {
  return makeError(0, 'network_error', '网络请求失败', null, cause)
}

function errorFromResponse(status, body) {
  const code = body && typeof body === 'object'
    ? String(body.code ?? body.error ?? `http_${status}`)
    : `http_${status}`
  const message = body && typeof body === 'object'
    ? String(body.message ?? body.error ?? `HTTP ${status}`)
    : `HTTP ${status}`
  return makeError(status, code, message, body && typeof body === 'object' ? body : null)
}

function requireId(value, field) {
  const id = String(value ?? '').trim()
  if (!id) throw makeError(0, 'invalid_argument', `${field} 不能为空`, { field })
  return id
}

/**
 * 通用请求：JSON body 默认序列化；raw = true 时 body 为原始字节流。
 * 204 → null；JSON 响应解包返回；非 2xx 抛带 status/code/data 的 Error；401 走全站拦截。
 */
async function request(method, path, body, { raw = false, contentType } = {}) {
  const options = { method, headers: {} }
  if (body !== undefined) {
    if (raw) {
      if (contentType) options.headers['Content-Type'] = contentType
      options.body = body
    } else {
      options.headers['Content-Type'] = 'application/json'
      options.body = JSON.stringify(body)
    }
  }
  let resp
  try {
    resp = await fetch(path, options)
  } catch (cause) {
    throw networkError(cause)
  }
  if (resp.status === 401) handleUnauthorized()
  if (!resp.ok) {
    let errBody = null
    try { errBody = await resp.json() } catch (e) { /* 非 JSON 错误体 */ }
    throw errorFromResponse(resp.status, errBody)
  }
  if (resp.status === 204) return null
  const ct = resp.headers.get('content-type') || ''
  if (ct.includes('application/json')) return resp.json()
  return resp
}

/** 预览播放时间（秒）→ 服务端精确帧 pts_us（合同 §1 frame 端点参数；负值截为 0）。
 *  只用于「按当前预览位置取帧」的粗定位；精确帧身份（展示序索引 ↔ PTS）以
 *  服务端 `/frames` 端点为准，前端不做任何固定步长/帧率估算。 */
export function ptsFromTime(seconds) {
  const t = Number(seconds)
  if (!Number.isFinite(t) || t <= 0) return 0
  return Math.round(t * 1e6)
}

/** 项目资源 URL（GET/PUT/DELETE 同形；不发请求）。 */
function projectUrlOf(packageId, projectId) {
  const pkg = encodeURIComponent(requireId(packageId, 'package_id'))
  const path = `${VIDEO_PROJECT_DIR}/${requireId(projectId, 'project_id')}.json`
  return `/api/packages/${pkg}/plugins/${encodeURIComponent(GAMER_VIDEO_PLUGIN_ID)}/resources/${path
    .split('/')
    .map(segment => encodeURIComponent(segment))
    .join('/')}`
}

/**
 * gamer.yaml 公开动作清单缝（Phase 7 §10.1）：视频侧对 YAML 栈的一切制作能力
 * 调用只经 POST /api/extensions/gamer.yaml/call（动作由 gamer.yaml 集中声明并
 * 分发），**禁止**直接写 gamer.yaml 私有目录或解析 YAML。返回 guest 结果的
 * `data` 信封（未包信封时按原结果兜底）。
 */
async function callGamerYamlAction(action, values) {
  const result = await request(
    'POST',
    `/api/extensions/${encodeURIComponent(GAMER_YAML_PLUGIN_ID)}/call`,
    { action, values },
  )
  return (result && typeof result === 'object' && result.data && typeof result.data === 'object')
    ? result.data
    : result
}

/**
 * 能力发现（简化计划 Phase 4/5）：GET /api/extensions/gamer.yaml/capabilities
 * → `{id, state, running, actions:[{action, version, surface, summary}]}`；
 * 404（未安装）→ `{id, running:false, actions:[]}` 兜底（调用方按不可用降级，
 * 不误报错误）。调用方据此决定 YAML 相关制作入口是否展示。
 */
async function gamerYamlCapabilities() {
  try {
    return await request('GET', `/api/extensions/${encodeURIComponent(GAMER_YAML_PLUGIN_ID)}/capabilities`)
  } catch (error) {
    if (error?.status === 404) {
      return { id: GAMER_YAML_PLUGIN_ID, state: null, running: false, actions: [] }
    }
    throw error
  }
}

export const videoApi = {
  // ---- 能力发现（简化计划 Phase 4/5）----

  /** gamer.yaml 公开动作清单与运行状态（YAML 制作入口可用性判定）。 */
  gamerYamlCapabilities,

  // ---- 媒体（合同 §1）----

  /** GET /api/media → `{"media":[MediaMetadata]}`（创建时间倒序）；便捷返回数组。 */
  listMedia: async () => {
    const rep = await request('GET', '/api/media')
    return Array.isArray(rep?.media) ? rep.media : []
  },

  // 注意：以下 promise 型方法一律 async——requireId 的参数校验 throw 必须变成
  // rejected promise（而非调用点同步抛出），调用方 `await`/`.rejects` 才能捕获。
  // mediaFileUrl / mediaFrameUrl 是同步 URL 构造器（模板/computed 直用），不在此列。

  /** GET /api/media/:id → MediaMetadata；404 `{"error":"media_not_found"}` 原样上抛。 */
  getMedia: async (id) => request('GET', `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}`),

  /** POST /api/media/import?name=<文件名>，raw 字节 body（组限额 1GiB）→ 201 MediaMetadata。 */
  importMedia: async (bytes, name) => request(
    'POST',
    `/api/media/import?name=${encodeURIComponent(requireId(name, 'name'))}`,
    bytes,
    { raw: true, contentType: 'application/octet-stream' },
  ),

  /** DELETE /api/media/:id → 204；被引用 409 `{"error":"media_referenced"}` 原样上抛。 */
  deleteMedia: async (id) => request('DELETE', `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}`),

  /** 原文件播放流 URL（`<video :src>` 用；支持 Range，不发请求）。 */
  mediaFileUrl: (id) => `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/file`,

  /**
   * 精确帧 PNG URL（`<img :src>` 用；同一请求逐字节可重复，不发请求）。
   * 参数（合同 §1）：ptsUs → `pts_us` 与 index → `index` 二选一（服务端 pts_us 优先），
   * maxWidth → `max_width` 最长边缩放上限。
   */
  mediaFrameUrl: (id, { ptsUs, index, maxWidth } = {}) => {
    const query = new URLSearchParams()
    if (ptsUs !== undefined && ptsUs !== null) query.set('pts_us', String(Math.max(0, Math.round(Number(ptsUs)))))
    if (index !== undefined && index !== null) query.set('index', String(Math.max(0, Math.round(Number(index)))))
    if (maxWidth !== undefined && maxWidth !== null) query.set('max_width', String(Math.max(1, Math.round(Number(maxWidth)))))
    const qs = query.toString()
    return `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/frame${qs ? `?${qs}` : ''}`
  },

  /**
   * 展示帧元信息（Phase 5）：GET /api/media/:id/frames
   * → `{media_id, frame_count, first_pts_us, last_pts_us, current?}`；
   * ptsUs 给出时返回「首个 pts ≥ 目标」的展示帧解析（`current:{index,pts_us}`）。
   */
  mediaFrames: async (id, { ptsUs } = {}) => {
    const base = `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/frames`
    const query = new URLSearchParams()
    if (ptsUs !== undefined && ptsUs !== null) query.set('pts_us', String(Math.max(0, Math.round(Number(ptsUs)))))
    const qs = query.toString()
    return request('GET', qs ? `${base}?${qs}` : base)
  },

  /**
   * 指定展示帧及相邻帧（Phase 5）：GET /api/media/:id/frames/:index
   * → `{index, pts_us, prev:{index,pts_us}|null, next:{index,pts_us}|null}`；
   * 越界 404 `frame_not_found` 原样上抛。逐帧步进的唯一权威实现（VFR/B 帧
   * 展示序由服务端归一，前端无 33ms 假设）。
   */
  mediaFrameNeighbors: async (id, index) => request(
    'GET',
    `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/frames/${Math.max(0, Math.round(Number(index) || 0))}`,
  ),

  // ---- 项目（Phase 6：Package 资源 API 承载，plugins/gamer.video/projects/<id>.json）----
  // 文本 JSON + 乐观并发 expected_version（Core 不解释内容；409/400 诊断原样上抛）。

  /** GET /api/packages/:pkg/plugins/gamer.video/resources?prefix=projects/ → 资源条目数组。 */
  listProjectEntries: async (packageId) => {
    const pkg = encodeURIComponent(requireId(packageId, 'package_id'))
    const base = `/api/packages/${pkg}/plugins/${encodeURIComponent(GAMER_VIDEO_PLUGIN_ID)}/resources`
    const rep = await request('GET', `${base}?prefix=${encodeURIComponent(`${VIDEO_PROJECT_DIR}/`)}`)
    return Array.isArray(rep?.resources) ? rep.resources : []
  },

  /** 项目资源 URL（GET/PUT/DELETE 同形；不发请求）。 */
  projectUrl: projectUrlOf,

  /** GET 项目 → 资源条目 `{content, version, path, updated_at, ...}`（含注记字段）。 */
  getProject: async (packageId, projectId) => request('GET', projectUrlOf(packageId, projectId)),

  /** PUT 项目：content 文本 + expected_version 乐观并发（force 显式跳过）。 */
  putProject: async (packageId, projectId, content, { expectedVersion, force } = {}) => request(
    'PUT',
    projectUrlOf(packageId, projectId),
    {
      content: requireId(content, 'content'),
      expected_version: expectedVersion || undefined,
      force: force === true || undefined,
    },
  ),

  /** DELETE 项目 → 204 → null。 */
  deleteProject: async (packageId, projectId) => request('DELETE', projectUrlOf(packageId, projectId)),

  /** POST rename：项目重命名（资源原子移动；body `{path, new_path}`）。 */
  renameProject: async (packageId, projectId, newProjectId) => {
    const pkg = encodeURIComponent(requireId(packageId, 'package_id'))
    const base = `/api/packages/${pkg}/plugins/${encodeURIComponent(GAMER_VIDEO_PLUGIN_ID)}/rename`
    return request('POST', base, {
      path: `${VIDEO_PROJECT_DIR}/${requireId(projectId, 'project_id')}.json`,
      new_path: `${VIDEO_PROJECT_DIR}/${requireId(newProjectId, 'new_project_id')}.json`,
    })
  },

  // ---- 录制（合同 §2）----

  /** POST /api/recording/start {device_id} → 202 RecordingSessionMeta；设备已有活动会话 409。 */
  recordingStart: async (deviceId) => request(
    'POST',
    '/api/recording/start',
    { device_id: requireId(deviceId, 'device_id') },
  ),

  /** POST /api/recording/:id/stop（合同 body「-」，无请求体）→ 200 终态 session（幂等）。 */
  recordingStop: async (id) => request('POST', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/stop`),

  /** POST /api/recording/:id/cancel（合同 body「-」）→ 200 终态（已落盘部分保留为 interrupted 素材）。 */
  recordingCancel: async (id) => request('POST', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/cancel`),

  /** GET /api/recording/:id → RecordingSessionMeta；404 `{"error":"recording_not_found"}`。 */
  recordingStatus: async (id) => request('GET', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}`),

  /** GET /api/recording/active?device_id= → session；404（无活动会话，轮询常态）→ null。 */
  activeRecording: async (deviceId) => {
    try {
      return await request(
        'GET',
        `/api/recording/active?device_id=${encodeURIComponent(requireId(deviceId, 'device_id'))}`,
      )
    } catch (error) {
      if (error && error.status === 404) return null
      throw error
    }
  },

  /** GET /api/recording/:id/events → `{"schema_version",events:[InputEventRecord]}`（时间轴升序）；便捷返回事件数组。 */
  recordingEvents: async (id) => {
    const rep = await request('GET', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/events`)
    return Array.isArray(rep?.events) ? rep.events : []
  },

  /**
   * 生成 YAML 草稿（合同 §5 + Phase 7 §10.3）：走动作清单缝
   * POST /api/extensions/gamer.yaml/call，action = automation.create_draft；
   * `comments` = 事件 id → 注释文本（服务端渲染为步骤上方注释行）。
   * 返回数据取 guest 结果的 `data` 字段
   * `{yaml, diagnostics:[{event_id,reason}], source:{recording_id,events}}`
   * （结果未包 data 信封时按原结果兜底）。草稿只是文本返回：不落盘、不执行。
   */
  createVideoDraft: async (recordingId, eventIds, comments = null) => {
    const values = {
      recording_id: requireId(recordingId, 'recording_id'),
      event_ids: (Array.isArray(eventIds) ? eventIds : []).map(id => String(id)),
    }
    if (comments && typeof comments === 'object') {
      values.comments = Object.fromEntries(
        Object.entries(comments).map(([id, text]) => [String(id), String(text ?? '')]),
      )
    }
    return callGamerYamlAction('automation.create_draft', values)
  },

  /**
   * 保存草稿为 automations 脚本（Phase 7 §10.3，动作清单 automation.save_draft）：
   * 服务端 v3 保存钩子校验（非 v3 结构化拒绝）+ 重名需 overwrite。
   * 返回 `{id:"<pkg>/<name>.yaml", path, package_id}`。
   */
  saveDraft: async ({ packageId, name, yaml, overwrite = false }) => callGamerYamlAction(
    'automation.save_draft',
    {
      package_id: requireId(packageId, 'package_id'),
      name: requireId(name, 'name'),
      yaml: String(yaml ?? ''),
      overwrite: overwrite === true,
    },
  ),

  /**
   * 从确定帧创建模板（Phase 7 §10.2，动作清单 template.create_from_frame）：
   * 帧上裁剪 PNG（base64）+ 相对区域交给 gamer.yaml（服务端命名规则 + 灰度
   * 归一化 + 短名冲突检测），并携带帧身份与校准元数据（来源追溯）。
   * 返回 `{name, short_name, path, size, region, frame, calibration}`。
   */
  createTemplateFromFrame: async ({
    packageId, name, pngBase64, region, preserveColor = false, overwrite = false, frame, calibration,
  }) => callGamerYamlAction('template.create_from_frame', {
    package_id: requireId(packageId, 'package_id'),
    name: requireId(name, 'name'),
    png_base64: requireId(pngBase64, 'png_base64'),
    region: region.map(v => Number(v)),
    preserve_color: preserveColor === true,
    overwrite: overwrite === true,
    frame,
    calibration,
  }),

  /**
   * 模板离线匹配测试（动作清单 vision.test_template = 复用 Core REST）：
   * media 模式给 `frame {mediaId, frameIndex?, ptsUs?}`（与 device 互斥；
   * 响应附帧身份 `frame` 字段）。region 为帧像素 [x,y,w,h]（可空 = 按模板名
   * 规则解析）。绝不触达设备/ADB。
   */
  visionTestTemplate: async ({ packageId, name, threshold, region = null, frame }) => {
    const body = {
      pkg: requireId(packageId, 'package_id'),
      plugin: GAMER_YAML_PLUGIN_ID,
      name: requireId(name, 'name'),
    }
    if (threshold !== undefined && threshold !== null && Number.isFinite(Number(threshold))) {
      body.threshold = Number(threshold)
    }
    if (Array.isArray(region) && region.length === 4) body.region = region.map(v => Math.max(0, Math.round(Number(v))))
    if (frame?.mediaId) {
      body.media_id = String(frame.mediaId)
      if (frame.frameIndex !== undefined && frame.frameIndex !== null) body.frame_index = Math.max(0, Math.round(Number(frame.frameIndex)))
      else if (frame.ptsUs !== undefined && frame.ptsUs !== null) body.pts_us = Math.max(0, Math.round(Number(frame.ptsUs)))
    }
    const result = await request('POST', '/api/capabilities/vision/test', body)
    return result
  },

  /**
   * 媒体引用全量替换（Phase 8 契约 §2.1，项目保存/素材移除时同步）：
   * POST /api/media/:id/refs，body `{refs:[{package_id, plugin_id, kind}]}`。
   * Workbench 与本模块之间也使用这组 REST 字段，避免跨层再定义一套引用模型；
   * camelCase 仅作为独立调用方的输入归一化，不改变线上字段契约。
   */
  setMediaRefs: async (id, refs) => request(
    'POST',
    `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/refs`,
    { refs: (Array.isArray(refs) ? refs : []).map(entry => ({
      package_id: requireId(entry?.package_id ?? entry?.packageId, 'package_id'),
      plugin_id: requireId(entry?.plugin_id ?? entry?.pluginId, 'plugin_id'),
      kind: requireId(entry?.kind, 'kind'),
    })) },
  ),
}
