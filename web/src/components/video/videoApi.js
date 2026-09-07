// 视频工作台 REST 封装（实施合同：docs/plans/gamer_video_workbench_contracts.md §1/§2/§6）。
//
// 合同 §4：为解除与 api.js（C 属地）的并行时序耦合，D2 在本目录自建 fetch 直调封装；
// 端点形态逐字对齐合同 §1（媒体）/§2（录制），草稿走既有扩展调用通路
// POST /api/extensions/gamer.yaml/call（action = automation.create_draft，合同 §5）。
// 同源 Cookie 鉴权默认携带（SameSite=Strict，不设 credentials、不引 CSRF token，
// 与 api.js/auth.js 同一口径）；401 交给 auth.js 全站拦截。
// 集成者后续可决定是否把本封装收编进 api.js。

import { handleUnauthorized } from '../../auth'

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

export const videoApi = {
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
   * 生成 YAML 草稿（合同 §5）：走既有扩展调用通路 POST /api/extensions/gamer.yaml/call，
   * action = automation.create_draft；返回数据取 guest 结果的 `data` 字段
   * `{yaml, diagnostics:[{event_id,reason}]}`（结果未包 data 信封时按原结果兜底）。
   * 草稿只是文本返回：不落盘、不执行。
   */
  createVideoDraft: async (recordingId, eventIds) => {
    const result = await request(
      'POST',
      '/api/extensions/gamer.yaml/call',
      {
        action: 'automation.create_draft',
        values: {
          recording_id: requireId(recordingId, 'recording_id'),
          event_ids: (Array.isArray(eventIds) ? eventIds : []).map(id => String(id)),
        },
      },
    )
    return (result && typeof result === 'object' && result.data && typeof result.data === 'object')
      ? result.data
      : result
  },
}
