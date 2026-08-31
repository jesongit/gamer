// System/Update API client（WEB-001；契约：release/contracts/system-api-v1.md，冻结）。
// fetch 封装与 ../api.js 完全同一风格：同源 Cookie 会话（fetch 不设 credentials，同源自动
// 携带）、401 交给 auth.handleUnauthorized 全站拦截、错误统一归一化为
// ApiError {status, code, message, details, data}。仅复用 ../api.js 的导出，不修改它。
//
// 端点（§0）：
//   GET  /api/system/info            需登录
//   GET  /api/system/update          需登录
//   POST /api/system/update/check    需登录 + 同源；202 = 受理（幂等）
//   POST /api/system/update/download 需登录 + 同源；202 = 受理（幂等）
//   POST /api/system/update/install  需登录 + 同源；202 = 受理（非幂等，并发第二个 409 update_busy）
//   POST /api/system/update/rollback 需登录 + 同源；202 = 受理（非幂等）
//   PUT  /api/system/update/policy   需登录 + 同源；整对象替换，幂等
//   GET  /health/ready               匿名（§8，向后兼容；重启等待期用它探测服务是否回来）
//
// 202 语义（§4.1）：受理即返回，body 冻结为 {update_id, state}；浏览器不能也不需要等一个
// 会因服务重启而断开的长 HTTP 请求，后续以轮询 GET /api/system/update 取进展。
import { ApiError } from '../api'
import { handleUnauthorized } from '../auth'
import { isUpdateState } from './states'

/**
 * §7 统一错误码常量表（11 个，冻结）：HTTP 状态码 + 是否可重试 + UI 提示文案。
 * UI 依据 retryable=false 的码（如 update_not_managed）隐藏/禁用对应按钮而非反复重试。
 */
export const SYSTEM_ERRORS = Object.freeze({
  update_not_managed:       { status: 409, retryable: false, hint: '当前部署模式不受升级器托管：Docker 请在宿主机更换镜像，直跑请手动替换程序' },
  update_busy:              { status: 409, retryable: true,  hint: '已有升级/回滚事务进行中，请等待其结束后再试' },
  update_not_available:     { status: 409, retryable: false, hint: '当前没有已验签的更新候选，请先执行检查更新' },
  update_not_ready:         { status: 409, retryable: true,  hint: '安装条件未满足（详见 blocking 门禁列表），满足后可重试' },
  signature_invalid:        { status: 422, retryable: false, hint: '发布清单验签失败，已拒绝该候选版本；等待新的正式版本后重新检查' },
  artifact_invalid:         { status: 422, retryable: true,  hint: '下载产物完整性校验失败，可重新下载修复传输损坏' },
  insufficient_space:       { status: 507, retryable: true,  hint: '磁盘空间不足，清理空间后重试' },
  schema_incompatible:      { status: 422, retryable: false, hint: '候选版本的数据 schema 超出当前程序可升级范围，需等待兼容的新版本' },
  launcher_unreachable:     { status: 502, retryable: true,  hint: '无法连接升级器（launcher 未运行或 IPC 不可用），恢复后有界重试' },
  rollback_unavailable:     { status: 409, retryable: false, hint: '没有可用的自动回滚点（自动回滚仅承诺提交之前的事务）' },
  manual_recovery_required: { status: 409, retryable: false, hint: '升级与自动回滚均失败，必须按维护手册人工恢复' },
})

export const SYSTEM_ERROR_CODES = Object.freeze(Object.keys(SYSTEM_ERRORS))

/** 非业务校验错误（§6，不计入 11 码）：PUT policy 字段非法 → 400 invalid_argument */
export const INVALID_ARGUMENT = 'invalid_argument'

const BASE = ''

function invalidResponse(message, data = null) {
  return new ApiError({ status: 502, code: 'invalid_response', message, data })
}

/** 非 2xx 响应 → 归一化 ApiError：业务错误 {code,message,details}；401/403 走中间件固定 body {error} */
async function errorFromResponse(r) {
  let body = null
  try { body = await r.json() } catch (e) { /* 非 JSON 错误响应 */ }
  const obj = body && typeof body === 'object' ? body : null
  return new ApiError({
    status: r.status,
    code: String(obj ? (obj.code ?? obj.error ?? `http_${r.status}`) : `http_${r.status}`),
    message: String(obj ? (obj.message ?? obj.error ?? `HTTP ${r.status}`) : `HTTP ${r.status}`),
    details: obj && obj.details && typeof obj.details === 'object' ? obj.details : null,
    data: body,
  })
}

async function request(method, path, body) {
  const r = await send(method, path, body)
  if (!r.ok) {
    if (r.status === 401) handleUnauthorized()
    throw await errorFromResponse(r)
  }
  return r
}

/** /health/ready 专用：503「未就绪」是探针的有效结论而非请求失败，原样返回响应（§8） */
async function requestTolerant(method, path, body) {
  return send(method, path, body)
}

async function send(method, path, body) {
  const opt = { method, headers: {} }
  if (body !== undefined) {
    opt.headers['Content-Type'] = 'application/json'
    opt.body = JSON.stringify(body)
  }
  let r
  try {
    r = await fetch(BASE + path, opt)
  } catch (e) {
    throw new ApiError({ status: 0, code: 'network_error', message: '网络请求失败', cause: e })
  }
  return r
}

async function reqJson(method, path, body) {
  const r = await request(method, path, body)
  const ct = r.headers.get('content-type') || ''
  if (!ct.includes('application/json')) throw invalidResponse('服务端响应不是 JSON')
  return r.json()
}

/**
 * 动作端点 202 受理响应校验（§4.1：body 冻结为 {update_id, state}）：
 * 结构不符视为契约破坏（invalid_response）；通过则追加 accepted:true 标记——
 * 表示「已受理进入后台协调器」，调用方不等待动作完成，以轮询取进展。
 */
function requireAccepted(rep) {
  if (!rep || typeof rep !== 'object'
    || typeof rep.update_id !== 'string' || !rep.update_id
    || !isUpdateState(rep.state)) {
    throw invalidResponse('动作受理响应缺少 update_id/state，或 state 不在 11 态枚举', rep)
  }
  return { ...rep, accepted: true }
}

export const systemApi = {
  /** §2 GET /api/system/info：版本/部署/schema/依赖/能力/启动信息 */
  getSystemInfo: () => reqJson('GET', '/api/system/info'),

  /** §3 GET /api/system/update：state/detail/update_id/candidate/progress/policy/last_error */
  getUpdateStatus: () => reqJson('GET', '/api/system/update'),

  /** §4 POST check：202 受理后台检查（幂等；返回 state=checking） */
  checkUpdate: async () => requireAccepted(await reqJson('POST', '/api/system/update/check', {})),

  /** §4 POST download：202 受理后台下载（幂等；返回 state=downloading 或 no-op 的 staged） */
  downloadUpdate: async () => requireAccepted(await reqJson('POST', '/api/system/update/download', {})),

  /**
   * §4 POST install：202 受理后台协调器（停机/快照/迁移/切换/重启）。
   * 返回 {accepted:true, update_id, state}；调用方不等待安装完成——期间 HTTP 服务会重启、
   * 连接会断开，断连不得判失败，重连后以 app.version/boot_id + update.state 判定结果。
   */
  installUpdate: async () => requireAccepted(await reqJson('POST', '/api/system/update/install', {})),

  /** §4 POST rollback：202 受理后台回滚（非幂等；并发第二个 409 update_busy） */
  rollbackUpdate: async () => requireAccepted(await reqJson('POST', '/api/system/update/rollback', {})),

  /** §6 PUT policy：整对象替换，幂等；200 回显保存后的策略对象 */
  setUpdatePolicy: (policy) => reqJson('PUT', '/api/system/update/policy', policy),

  /**
   * §8 /health/ready：匿名轻量探针。就绪 200 / 未就绪 503 都按「结论」resolve body
   *（调用方读 body.ready 判定），仅网络失败抛 ApiError；升级重启等待期用于探测恢复。
   */
  getHealthReady: async () => {
    const r = await requestTolerant('GET', '/health/ready')
    try { return await r.json() } catch (e) { return null }
  },
}
