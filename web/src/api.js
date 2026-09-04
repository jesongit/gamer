// 当前后端 API 封装（Rust 服务端）。
// 所有受保护请求遇到 401 都交给 Cookie 会话层处理；资源与运行接口不保留旧契约降级。
import { handleUnauthorized } from './auth'

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

  // 按键映射（data/<pkg>/keymap；资源 id 为当前分区内的方案名）
  listKeymaps: (pkg) => req('GET', `/api/keymaps?pkg=${encodeURIComponent(requireId(pkg, 'pkg'))}`),
  getKeymap: (name, pkg) => req(
    'GET',
    `/api/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
  ),
  createKeymap: ({ pkg, name, content } = {}) => req('POST', '/api/keymaps', { pkg, name, content }),
  updateKeymap: async (name, pkg, payload = {}) => req(
    'PUT',
    `/api/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
    updateBody(payload, keymapId(name, pkg)),
  ),
  deleteKeymap: (name, pkg) => req(
    'DELETE',
    `/api/keymaps/${encodeURIComponent(keymapId(name, pkg))}`,
  ),

  // 模板（按应用分区 data/<pkg>/tmpl；pkg 缺省=跨分区全列）
  listTemplates: (pkg) => req('GET', `/api/templates${pkg ? `?pkg=${encodeURIComponent(pkg)}` : ''}`),
  // 创建只接受短名；region 存在时由服务端组合带区域元数据的完整文件名。
  // preserveColor=true 时由服务端追加 #1 文件名标记并保留颜色通道；默认灰度压缩。
  createTemplate: (shortName, dataB64, pkg, region, preserveColor = false) => req('POST', '/api/templates', {
    short_name: shortName,
    ...(region !== undefined ? { region } : {}),
    ...(preserveColor ? { grayscale_only: false } : {}),
    data_b64: dataB64,
    pkg,
  }),
  // 图片替换与创建严格分离：名称/分区来自 URL/query，body 只有 data_b64。
  replaceTemplateImage: (name, dataB64, pkg) =>
    req('PUT', `/api/templates/${encodeURIComponent(name)}/image?pkg=${encodeURIComponent(pkg)}`, { data_b64: dataB64 }),
  renameTemplate: (oldName, newName, pkg) =>
    req('PUT', `/api/templates/${encodeURIComponent(oldName)}?pkg=${encodeURIComponent(pkg)}`, { name: newName }),
  deleteTemplate: (name, pkg) =>
    req('DELETE', `/api/templates/${encodeURIComponent(name)}?pkg=${encodeURIComponent(pkg)}`),
  testTemplate: (name, deviceId, threshold, region, pkg) =>
    req('POST', `/api/templates/${encodeURIComponent(name)}/test`, { device_id: deviceId, threshold, region, pkg }),
  // 模板缩略图/预览 URL（<img :src> 用；pkg 必填）
  tplImageUrl: (name, pkg) => `/api/templates/${encodeURIComponent(name)}/image?pkg=${encodeURIComponent(pkg)}`,

  // 脚本（id 形如 "<pkg>/<name>.yaml"，含 '/'，拼 URL 必须整体 encodeURIComponent）
  listScripts: () => req('GET', '/api/scripts'),
  // 单脚本读取（含内容版本短码 version：编辑器 expected_version 冲突检测依据）
  getScript: (id) => req('GET', `/api/scripts/${encodeURIComponent(requireId(id, 'script_id'))}`),
  // POST 只创建；PUT 只更新。更新缺版本时在客户端拒绝，force 必须显式为 true。
  createScript: ({ name, content, pkg } = {}) => req('POST', '/api/scripts', { name, content, pkg }),
  updateScript: async (id, payload = {}) => req(
    'PUT',
    `/api/scripts/${encodeURIComponent(requireId(id, 'script_id'))}`,
    updateBody(payload, id),
  ),
  deleteScript: (id) => req('DELETE', `/api/scripts/${encodeURIComponent(id)}`),
  // 函数库（data/<pkg>/func/；id 形如 "<pkg>/<文件短路径>.yaml"，整体 encodeURIComponent。
  // 不进脚本列表/运行接口/任务选择器；GET 单文件含 content/version/functions（顶层函数名清单））
  listFunctions: (pkg) => req('GET', `/api/functions?pkg=${encodeURIComponent(pkg)}`),
  getFunction: (id) => req('GET', `/api/functions/${encodeURIComponent(id)}`),
  // POST 只创建；PUT 只更新/重命名，更新缺版本时在客户端拒绝。
  createFunction: ({ pkg, name, content } = {}) => req('POST', '/api/functions', { pkg, name, content }),
  updateFunction: async (id, payload = {}) => req(
    'PUT',
    `/api/functions/${encodeURIComponent(requireId(id, 'function_id'))}`,
    updateBody(payload, id),
  ),
  deleteFunction: (id) => req('DELETE', `/api/functions/${encodeURIComponent(id)}`),

  // 脚本运行（阶段 5 契约）：body {device_id, start_index?, args?}——args 为稀疏显式覆盖映射
  //（bool/coord/time/color/tmpl/key/text 七类；「使用默认值」的参数省略，由服务端解析默认值）。
  // 成功 202 {run_id, state, resolved_args}；参数诊断 400 {error:"invalid_args", diagnostics:[...]}
  //（err.status/err.data 可取）；设备占用 409 {error:"device_busy", run_id, script_id, source, started_at}
  runScript: async (id, deviceId, startIndex = 0, args) =>
    requireRunResponse(await req('POST', `/api/scripts/${encodeURIComponent(requireId(id, 'script_id'))}/run`, {
      device_id: deviceId,
      start_index: startIndex,
      ...(args && Object.keys(args).length ? { args } : {}),
    })),
  // 函数测试（阶段 5）：id = 函数库文件 id（"<pkg>/<文件短路径>.yaml"，整体 encodeURIComponent）。
  // body {device_id, function?, start_index?, args?}（function 缺省 = 文件第一个函数）；
  // 响应/错误语义与脚本 run 相同（RunManager 统一 run_id 管理）
  runFunction: async (id, deviceId, opts = {}) =>
    requireRunResponse(await req('POST', `/api/functions/${encodeURIComponent(requireId(id, 'function_id'))}/run`, {
      device_id: deviceId,
      ...(opts.function ? { function: opts.function } : {}),
      ...(opts.start_index !== undefined ? { start_index: opts.start_index } : {}),
      ...(opts.args && Object.keys(opts.args).length ? { args: opts.args } : {}),
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

  // 定时任务（阶段 5 参数化）：创建/更新接受 args（稀疏显式覆盖映射，服务端解析为完整
  // 快照存储并计算 param_signature）；列表响应含 args 视图 / param_signature / param_stale。
  // PUT/更新签名不匹配且无 reconfirm:true → 409 {code:"param_signature_conflict"}；
  // 带 reconfirm 则按当前参数声明重算快照
  listTasks: () => req('GET', '/api/tasks'),
  // 任务详情（args 解析视图所在端点；列表仅带 param_stale/has_args/param_signature）
  getTask: (id) => req('GET', `/api/tasks/${id}`),
  saveTask: (t) => req('POST', '/api/tasks', t),
  deleteTask: (id) => req('DELETE', `/api/tasks/${id}`),
  // 任务立即执行（用任务已存参数快照；过期/无快照由服务端明确报错）：202 {run_id}
  runTaskNow: async (id) => requireRunResponse(await req('POST', `/api/tasks/${id}/run`)),

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
