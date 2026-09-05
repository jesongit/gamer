// 当前后端 API 封装（Rust 服务端）。
// 所有受保护请求遇到 401 都交给 Cookie 会话层处理；资源与运行接口不保留旧契约降级。
//
// P11.6 通用资源 API：脚本/函数库/模板/按键映射统一走
// `/api/apps/:app/resources[...]`（kind ∈ scripts|functions|templates|keymaps|...）；
// 运行统一走 POST /api/runs（runner_id + entrypoint + payload，ADR-12/13：执行
// 目标按 runner 分发）。本封装只提供 runner 无关的通用 run()；具体 runner 的
// 包装（如 YAML 自动化 runner）归扩展前端侧（gamer-yaml-runner.js 等），Core API 层
// 不认识任何 runner 注册 id。
import { handleUnauthorized } from './auth'

/** base64 → Uint8Array（模板原始字节上传用） */
function base64ToBytes(dataB64) {
  const binary = atob(dataB64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
  return bytes
}

/** 客户端组合模板完整文件名：短名 + #x1_y1_x2_y2 区域后缀 + #1 颜色标记 + .png。
 * 与服务端 matcher tpl_region_from_name 同编码（区域 ×1000 三位整数）。 */
function composeTemplateName(shortName, region, preserveColor) {
  const raw = String(shortName || '').trim()
  const lower = raw.toLowerCase()
  const stem = lower.endsWith('.png') ? raw.slice(0, -4) : raw
  let name = stem
  if (Array.isArray(region) && region.length === 4) {
    const toInt3 = v => String(Math.min(999, Math.round(v * 1000))).padStart(3, '0')
    name += `#${region.map(toInt3).join('_')}`
  }
  if (preserveColor) name += '#1'
  return `${name}.png`
}

const BASE = ''

/** 所有 API 失败的稳定错误形态，供视图只按 code/status/data 判断。 */
export class ApiError extends Error {
  constructor({ status = 0, code = 'unknown_error', message = '请求失败', data = null, details = null, cause } = {}) {
    super(message, cause ? { cause } : undefined)
    this.name = 'ApiError'
    this.status = status
    this.code = code
    this.data = data
    this.details = details
  }
}

async function errorFromResponse(r) {
  let body = null
  try { body = await r.json() } catch (e) { /* 非 JSON 错误响应 */ }
  const code = body && typeof body === 'object'
    ? String(body.code ?? body.error ?? `http_${r.status}`)
    : `http_${r.status}`
  const details = body && typeof body === 'object' ? (body.diagnostics ?? null) : null
  const diagnostic = Array.isArray(details)
    ? details.find(d => d && typeof d === 'object' && typeof d.message === 'string' && d.message)
    : null
  const diagnosticMessage = diagnostic
    ? `${diagnostic.message}${diagnosticLocation(diagnostic) ? `（${diagnosticLocation(diagnostic)}）` : ''}`
    : null
  const message = body && typeof body === 'object'
    ? String(body.message ?? diagnosticMessage ?? body.error ?? `HTTP ${r.status}`)
    : `HTTP ${r.status}`
  return new ApiError({ status: r.status, code, message, data: body, details })
}

function networkError(cause) {
  return new ApiError({ status: 0, code: 'network_error', message: '网络请求失败', cause })
}

function diagnosticLocation(diagnostic) {
  const path = diagnostic?.step_path ? String(diagnostic.step_path) : ''
  const field = diagnostic?.field ? String(diagnostic.field) : ''
  const alreadyIncludesField = path === field || path.endsWith(`.${field}`)
  return [path, alreadyIncludesField ? '' : field].filter(Boolean).join('.')
}

function invalidResponse(message, data = null) {
  return new ApiError({ status: 502, code: 'invalid_response', message, data })
}

function requireId(value, field) {
  if (typeof value !== 'string' || !value.trim()) {
    throw new ApiError({
      status: 0,
      code: 'invalid_argument',
      message: `${field} 不能为空`,
      data: { field },
    })
  }
  return value
}

function requireRunResponse(rep) {
  if (!rep || typeof rep !== 'object' || typeof rep.run_id !== 'string' || !rep.run_id) {
    throw invalidResponse('服务端运行响应缺少 run_id', rep)
  }
  return rep
}

function keymapId(value, pkg) {
  const id = requireId(value, 'keymap')
  return id.includes('/') ? id : `${requireId(pkg, 'pkg')}/${id}`
}

/** Content-Disposition: attachment; filename="<id>-<version>.gamerpkg" → 文件名（解析不出返回空串） */
function filenameFromDisposition(value) {
  const m = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(String(value || ''))
  return m ? m[1] : ''
}

function requireDeviceRunResponse(rep) {
  if (rep && rep.active === false) return rep
  if (rep && rep.active === true && rep.run && typeof rep.run === 'object' && rep.run.run_id) return rep
  throw invalidResponse('服务端设备运行响应不符合当前契约', rep)
}

function updateBody({ content, name, expected_version, force } = {}, resource) {
  const body = { content }
  if (name !== undefined) body.name = name
  if (force === true) {
    body.force = true
    if (expected_version !== undefined) body.expected_version = expected_version
    return body
  }
  if (typeof expected_version !== 'string' || !expected_version) {
    throw new ApiError({
      status: 409,
      code: 'version_required',
      message: '更新资源必须提供 expected_version，或显式 force:true',
      data: { code: 'version_required', resource, field: 'expected_version' },
    })
  }
  body.expected_version = expected_version
  return body
}

async function readResult(r) {
  const ct = r.headers.get('content-type') || ''
  if (ct.includes('application/json')) return r.json()
  return r
}

async function response(method, path, body, extra = {}) {
  const { rawBody = false, ...fetchOptions } = extra
  const opt = { method, headers: {} }
  if (body !== undefined) {
    if (!fetchOptions.headers) opt.headers['Content-Type'] = 'application/json'
    opt.body = rawBody ? body : (typeof body === 'string' ? body : JSON.stringify(body))
  }
  Object.assign(opt, fetchOptions)
  let r
  try {
    r = await fetch(BASE + path, opt)
  } catch (e) {
    throw networkError(e)
  }
  if (!r.ok) {
    if (r.status === 401) handleUnauthorized()
    throw await errorFromResponse(r)
  }
  return r
}

async function req(method, path, body) {
  return readResult(await response(method, path, body))
}

function base64Utf8(value) {
  const bytes = new TextEncoder().encode(value)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

function extensionUploadOptions(options = {}) {
  const headers = { 'Content-Type': 'application/zip' }
  if (options.source === 'official') headers['X-Gamer-Extension-Source'] = 'official'
  if (options.registryProof) {
    headers['X-Gamer-Registry-Proof'] = typeof options.registryProof === 'string'
      ? options.registryProof
      : base64Utf8(JSON.stringify(options.registryProof))
  }
  if (options.permissionConfirmed === true) headers['X-Gamer-Permission-Confirm'] = '1'
  return { rawBody: true, headers }
}

export const api = {
  // 登录/会话/退出见 src/auth.js（阶段 2 Cookie 会话；本封装不持有认证端点）

  // 扩展生命周期与动态 UI contribution
  listExtensions: () => req('GET', '/api/extensions'),
  listExtensionUi: () => req('GET', '/api/extensions/ui'),
  // Phase 10 插件管理：归档始终以 application/zip 上传，服务端重新验证
  // 来源、Registry proof、包签名与权限确认；URL 不会作为 iframe 来源传入。
  getExtensionManagement: () => req('GET', '/api/extensions/management'),
  inspectExtension: async (file, options = {}) => {
    const r = await response(
      'POST', '/api/extensions/inspect', file,
      extensionUploadOptions(options),
    )
    return readResult(r)
  },
  installExtension: async (file, options = {}) => {
    const r = await response(
      'POST', '/api/extensions', file,
      extensionUploadOptions(options),
    )
    return readResult(r)
  },
  updateExtension: async (id, file, options = {}) => {
    const pluginId = requireId(id, 'extension_id')
    const r = await response(
      'POST', `/api/extensions/${encodeURIComponent(pluginId)}/update`, file,
      extensionUploadOptions(options),
    )
    return readResult(r)
  },
  enableExtension: (id) => req('POST', `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/enable`, {}),
  disableExtension: (id) => req('POST', `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/disable`, {}),
  startExtension: (id, app_context) => req('POST', `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/start`, app_context ? { app_context } : {}),
  stopExtension: (id) => req('POST', `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/stop`, {}),
  // declarative 面板 plugin.call：action 必须在插件 manifest 的声明按钮集合内（服务端校验）
  callExtension: (id, action, values = {}) => req(
    'POST',
    `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/call`,
    { action, values },
  ),
  // 版本切换（含回滚）：把已安装的某个版本设为活动版本；插件 Running 时服务端 409，
  // 版本未安装 404，成功返回 { id, active_version, state }
  activateExtension: (id, version) => req(
    'POST',
    `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/activate`,
    { version: requireId(version, 'extension_version') },
  ),
  uninstallExtension: (id, version, { deleteData = false } = {}) => req(
    'DELETE',
    `/api/extensions/${encodeURIComponent(requireId(id, 'extension_id'))}/${encodeURIComponent(requireId(version, 'extension_version'))}?delete_data=${deleteData ? '1' : '0'}`,
  ),

  // 设备
  listDevices: () => req('GET', '/api/devices'),
  scanDevices: () => req('POST', '/api/devices/scan'),
  createDevice: (d) => req('POST', '/api/devices', d),
  updateDevice: (id, d) => req('PUT', `/api/devices/${id}`, d),
  deleteDevice: (id) => req('DELETE', `/api/devices/${id}`),
  connectDevice: (id) => req('POST', `/api/devices/${id}/connect`),
  disconnectDevice: (id) => req('POST', `/api/devices/${id}/disconnect`),
  screenshot: async (id) => {
    const r = await req('POST', `/api/devices/${id}/screenshot`)
    const blob = await r.blob()
    return new Promise((resolve, reject) => {
      const fr = new FileReader()
      fr.onload = () => resolve(fr.result)
      fr.onerror = reject
      fr.readAsDataURL(blob)
    })
  },
  control: (id, cmd) => req('POST', `/api/devices/${id}/control`, cmd),
  listApps: (id) => req('GET', `/api/devices/${id}/apps`),
  listAppsByAddr: (addr) => req('GET', `/api/apps?addr=${encodeURIComponent(addr)}`),

  // 按键映射（通用资源 API keymaps kind；资源 id = "<pkg>/<方案名>.yaml"）
  listKeymaps: (pkg) => req('GET', `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/keymaps`),
  getKeymap: (name, pkg) => req(
    'GET',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
  ),
  createKeymap: ({ pkg, name, content } = {}) => req(
    'POST',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/keymaps`,
    { name, content },
  ),
  updateKeymap: async (name, pkg, payload = {}) => req(
    'PUT',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
    updateBody(payload, keymapId(name, pkg)),
  ),
  deleteKeymap: (name, pkg) => req(
    'DELETE',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
  ),

  // 模板（通用资源 API templates kind；pkg 缺省 = "-" 跨分区通配）
  listTemplates: (pkg) => req(
    'GET',
    `/api/apps/${pkg ? encodeURIComponent(pkg) : '-'}/resources/templates`,
  ),
  // 客户端组合完整文件名（短名 + #区域后缀 + #1 颜色标记），原始字节上传；
  // 灰度重编码由服务端按文件名 #1 标记决定。
  createTemplate: async (shortName, dataB64, pkg, region, preserveColor = false) => {
    const app = encodeURIComponent(requireId(pkg, 'pkg'))
    const name = composeTemplateName(shortName, region, preserveColor)
    const r = await response(
      'POST',
      `/api/apps/${app}/resources/templates?name=${encodeURIComponent(name)}`,
      base64ToBytes(dataB64),
      { rawBody: true, headers: { 'Content-Type': 'image/png' } },
    )
    return readResult(r)
  },
  // 图片替换：名称/分区来自 URL，body 只有原始图片字节。
  replaceTemplateImage: async (name, dataB64, pkg) => {
    const app = encodeURIComponent(requireId(pkg, 'pkg'))
    const r = await response(
      'PUT',
      `/api/apps/${app}/resources/templates/${encodeURIComponent(name)}`,
      base64ToBytes(dataB64),
      { rawBody: true, headers: { 'Content-Type': 'image/png' } },
    )
    return readResult(r)
  },
  // 重命名：JSON {name}；服务端经扩展内容钩子同步改写脚本/函数引用。
  renameTemplate: (oldName, newName, pkg) =>
    req(
      'PUT',
      `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/templates/${encodeURIComponent(oldName)}`,
      { name: newName },
    ),
  deleteTemplate: (name, pkg) =>
    req(
      'DELETE',
      `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/templates/${encodeURIComponent(name)}`,
    ),
  // 模板匹配测试 = vision 能力位语义
  testTemplate: (name, deviceId, threshold, region, pkg) =>
    req('POST', '/api/capabilities/vision/test', { device_id: deviceId, threshold, region, pkg, name }),
  // 模板缩略图/预览 URL（<img :src> 用；pkg 必填）
  tplImageUrl: (name, pkg) => `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/templates/${encodeURIComponent(name)}`,

  // 脚本（通用资源 API scripts kind；id 形如 "<pkg>/<name>.yaml"，含 '/'，
  // 拼 URL 必须整体 encodeURIComponent；app 段传 "-" 由 id 自带分区）
  listScripts: () => req('GET', '/api/apps/-/resources/scripts'),
  // 单脚本读取（含内容版本短码 version：编辑器 expected_version 冲突检测依据）
  getScript: (id) => req(
    'GET',
    `/api/apps/-/resources/scripts/${encodeURIComponent(requireId(id, '资源 id'))}`,
  ),
  // POST 只创建；PUT 只更新。更新缺版本时在客户端拒绝，force 必须显式为 true。
  createScript: ({ name, content, pkg } = {}) => req(
    'POST',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/scripts`,
    { name, content },
  ),
  updateScript: async (id, payload = {}) => req(
    'PUT',
    `/api/apps/-/resources/scripts/${encodeURIComponent(requireId(id, '资源 id'))}`,
    updateBody(payload, id),
  ),
  deleteScript: (id) => req(
    'DELETE',
    `/api/apps/-/resources/scripts/${encodeURIComponent(id)}`,
  ),
  // 函数库（通用资源 API functions kind；id 形如 "<pkg>/<文件短路径>.yaml"，
  // 整体 encodeURIComponent。不进脚本列表/运行接口/任务选择器；GET 单文件含
  // content/version/functions（顶层函数名清单，扩展注记提供））
  listFunctions: (pkg) => req(
    'GET',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/functions`,
  ),
  getFunction: (id) => req(
    'GET',
    `/api/apps/-/resources/functions/${encodeURIComponent(id)}`,
  ),
  // POST 只创建；PUT 只更新/重命名，更新缺版本时在客户端拒绝。
  createFunction: ({ pkg, name, content } = {}) => req(
    'POST',
    `/api/apps/${encodeURIComponent(requireId(pkg, 'pkg'))}/resources/functions`,
    { name, content },
  ),
  updateFunction: async (id, payload = {}) => req(
    'PUT',
    `/api/apps/-/resources/functions/${encodeURIComponent(requireId(id, '资源 id'))}`,
    updateBody(payload, id),
  ),
  deleteFunction: (id) => req(
    'DELETE',
    `/api/apps/-/resources/functions/${encodeURIComponent(id)}`,
  ),

  // 统一执行入口（P11.6 / ADR-12）：POST /api/runs {runner_id, entrypoint,
  // device_id, payload}——runner_id 为 runner 注册 id（分发目标），entrypoint 为
  // runner 私有寻址，payload 为 runner 私有不透明值；本方法对具体 runner 保持
  // 无知（具体 runner 的包装见扩展前端侧 gamer-yaml-runner.js）。
  // 成功 202 {run_id, state, resolved_args}；参数诊断 400 {error:"invalid_args",
  // diagnostics:[...]}；设备占用 409 {error:"device_busy", ...}；运行依赖缺失
  //（runner 未注册）424 {code:"dependency_unavailable"}
  run: async ({ runner_id, entrypoint, device_id, payload } = {}) =>
    requireRunResponse(await req('POST', '/api/runs', {
      runner_id: requireId(runner_id, 'runner_id'),
      entrypoint: requireId(entrypoint, 'entrypoint'),
      device_id: device_id,
      payload: payload && typeof payload === 'object' ? payload : {},
    })),
  // 统一运行实例（run_id 主键）：单次查询 RunRecord / 按次取消（终态以查询为准）
  getRun: async (runId) => requireRunResponse(await req('GET', `/api/runs/${encodeURIComponent(requireId(runId, 'run_id'))}`)),
  cancelRun: async (runId) => {
    const id = requireId(runId, 'run_id')
    return req('POST', `/api/runs/${encodeURIComponent(id)}/cancel`)
  },
  // 设备当前运行中的脚本（页面刷新后恢复运行态用）
  // 当前契约 → {active:true,run:RunRecord} | {active:false}。
  deviceRun: async (id) => requireDeviceRunResponse(await req('GET', `/api/devices/${id}/run`)),

  // 统一任务 API（P11.1 / ADR-12：Task = 任意 ScheduleProvider + 任意 Runner）。
  // JSON 形状：runner 嵌套 {runner_id, entrypoint, payload}（payload 为 runner
  // 私有不透明值，允许保存未知 runner）；schedule = {provider_id, config}
  // （内置 gamer.cron → provider_id "cron"，config.expression 为 cron 表达式）。
  // state ∈ active | suspended | cancelled | dependency_missing（依赖缺失任务
  // 保留，等待恢复）；enable/disable 为显式状态迁移端点。
  listTasks: () => req('GET', '/api/tasks'),
  // 任务详情（与列表同形状；详情与列表无字段差异）
  getTask: (id) => req('GET', `/api/tasks/${id}`),
  saveTask: (t) => req('POST', '/api/tasks', t),
  updateTask: (id, t) => req('PUT', `/api/tasks/${id}`, t),
  deleteTask: (id) => req('DELETE', `/api/tasks/${id}`),
  // 任务立即执行：202 {run_id}；设备占用 409 device_busy；运行依赖缺失
  //（runner/schedule provider/脚本不存在）424 {code:"dependency_unavailable"}，
  // 任务随之进入 dependency_missing 状态
  runTaskNow: async (id) => requireRunResponse(await req('POST', `/api/tasks/${id}/run`)),
  // 启用/停用调度：显式状态迁移（enable 重算唤醒游标；disable 挂起并记 "disabled"）
  enableTask: (id) => req('POST', `/api/tasks/${id}/enable`),
  disableTask: (id) => req('POST', `/api/tasks/${id}/disable`),
  // 挂起（带原因，任务保留）/ 恢复（= enable 语义：重算唤醒、清 reason）/ 取消调度
  //（终态 cancelled，不再排程）。suspend/resume/cancel 返回迁移后的任务 JSON。
  suspendTask: (id, reason = 'suspended') => req('POST', `/api/tasks/${id}/suspend`, { reason }),
  resumeTask: (id) => req('POST', `/api/tasks/${id}/resume`),
  cancelTask: (id) => req('POST', `/api/tasks/${id}/cancel`),
  // UI 支撑只读端点：已注册 runner / schedule provider（执行器与触发方式下拉）
  listRunners: () => req('GET', '/api/runners'),
  listScheduleProviders: () => req('GET', '/api/schedule-providers'),

  // ---- App Package（游戏包）与本地编辑区（workspace）----
  // 已装游戏包列表：{packages:[{id,name,active_version(null=无激活),android_packages,versions:[...]}]}
  listAppPackages: () => req('GET', '/api/app-packages'),
  // 安装 .gamerpkg 归档原始字节（Content-Type: application/zip）；expectedSha256 提供时
  // 带 X-Expected-Sha256 校验头（不匹配 400）。成功 201 返回包 JSON；409 = 同 id+version
  // 已安装或 primary 冲突（同一安卓应用已有激活内容包），错误体 {error} 文本区分
  installAppPackage: async (bytes, expectedSha256) => {
    const headers = { 'Content-Type': 'application/zip' }
    if (expectedSha256) headers['X-Expected-Sha256'] = String(expectedSha256)
    const r = await response('POST', '/api/app-packages/install', bytes, { rawBody: true, headers })
    return readResult(r)
  },
  // 本地编辑区导出为 .gamerpkg：200 二进制 + Content-Disposition 文件名 + X-Content-Sha256；
  // 404 = 工作区没有 package.toml（先 PUT workspace 初始化）；400 {code:"preflight_failed"}
  // 错误体 error 为逐行问题列表
  exportAppPackage: async (androidPackage) => {
    const r = await req('POST', '/api/app-packages/export', {
      android_package: requireId(androidPackage, 'android_package'),
    })
    const blob = await r.blob()
    return {
      blob,
      filename: filenameFromDisposition(r.headers.get('content-disposition')),
      sha256: r.headers.get('x-content-sha256') || '',
    }
  },
  // 把已安装的某版本游戏包导入到指定安卓应用的本地编辑区（替换现场资源）；
  // 400 = target 不在该包 android.packages 或提取后校验失败，404 = 包/版本不存在
  editAppPackage: (id, version, androidPackage) => req(
    'POST',
    `/api/app-packages/${encodeURIComponent(requireId(id, 'package_id'))}/${encodeURIComponent(requireId(version, 'package_version'))}/edit`,
    { android_package: requireId(androidPackage, 'android_package') },
  ),
  // 本地编辑区（data/<android 包名>/）元数据（package.toml；null = 未初始化）+ 六目录资源统计
  getWorkspace: (androidPackage) => req(
    'GET',
    `/api/workspace/${encodeURIComponent(requireId(androidPackage, 'android_package'))}`,
  ),
  // 保存工作区元数据（服务端 deny_unknown_fields：只发 id?/version/name?/android_packages；
  // name 为空串时整个省略——服务端拒绝空名称）
  saveWorkspace: (androidPackage, payload = {}) => req(
    'PUT',
    `/api/workspace/${encodeURIComponent(requireId(androidPackage, 'android_package'))}`,
    payload,
  ),

  // 日志
  listLogs: (deviceId, level, limit) => {
    const p = new URLSearchParams()
    if (deviceId) p.set('device_id', deviceId)
    if (level) p.set('level', level)
    if (limit) p.set('limit', limit)
    return req('GET', `/api/logs?${p.toString()}`)
  },
  clearLogs: () => req('DELETE', '/api/logs')
}
