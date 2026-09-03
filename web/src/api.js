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
  exportKeymaps: async (pkg) => {
    const r = await response('GET', `/api/keymaps/export?pkg=${encodeURIComponent(requireId(pkg, 'pkg'))}`)
    const cd = r.headers.get('content-disposition') || ''
    let filename = ''
    const m = cd.match(/filename\*=UTF-8''([^;\s]+)/) || cd.match(/filename="?([^";\s]+)"?/)
    if (m) { try { filename = decodeURIComponent(m[1]) } catch (e) { filename = m[1] } }
    return { blob: await r.blob(), filename }
  },
  importKeymaps: async (file, confirm, pkg) => {
    const r = await response(
      'POST',
      `/api/keymaps/import?confirm=${confirm ? 1 : 0}&pkg=${encodeURIComponent(requireId(pkg, 'pkg'))}`,
      file,
      { rawBody: true, headers: { 'Content-Type': 'application/zip' } },
    )
    return readResult(r)
  },

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
  // 导出整分区快照 zip（yaml/ + tmpl/ 全量，?pkg= 指定分区）→ { blob, filename }
  exportPartition: async (pkg) => {
    const r = await response('GET', `/api/scripts/export?pkg=${encodeURIComponent(pkg)}`)
    const cd = r.headers.get('content-disposition') || ''
    let filename = ''
    const m = cd.match(/filename\*=UTF-8''([^;\s]+)/) || cd.match(/filename="?([^";\s]+)"?/)
    if (m) { try { filename = decodeURIComponent(m[1]) } catch (e) { filename = m[1] } }
    return { blob: await r.blob(), filename }
  },
  // 导入分区快照 zip 到指定应用分区：confirm=false dry-run 只解析报告，true 落盘（同名替换）
  importScripts: async (file, confirm, pkg) => {
    const r = await response(
      'POST',
      `/api/scripts/import?confirm=${confirm ? 1 : 0}&pkg=${encodeURIComponent(pkg)}`,
      file,
      { rawBody: true, headers: { 'Content-Type': 'application/zip' } },
    )
    return readResult(r)
  },

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

// ---- 分区快照导入（服务端 ImportReport 契约，scripts.rs ImportReport 同构）----
// POST /api/scripts/import?pkg=<分区>：confirm 缺省（或非 1/true）= dry-run，只解析不落盘，
// 返回 {scripts,functions,templates} 三类资源同构报告，每类
// {add:[zip相对路径], overwrite:[同名将被覆盖路径], invalid:[{path,reason}]}；
// confirm=1 落盘（报告同构，add/overwrite 变为实际结果），任一 invalid 整体拒绝（400 {error}）。

// 归一化 ImportReport：三类资源合并为统一 {add, overwrite, invalid} 列表；
// 形态不符（未来端点契约再变）返回 null —— 调用方必须按错误处理，不得当「无冲突」放行直接导入。
export function summarizeImportReport(rep) {
  const buckets = rep && typeof rep === 'object' ? [rep.scripts, rep.functions, rep.templates] : []
  if (buckets.length !== 3 || buckets.some(b => !isReportBucket(b))) return null
  return {
    add: buckets.flatMap(b => b.add),
    overwrite: buckets.flatMap(b => b.overwrite),
    invalid: buckets.flatMap(b => b.invalid).map(e => ({ path: String(e.path), reason: String(e.reason ?? '') })),
  }
}

function isReportBucket(b) {
  return !!b && typeof b === 'object'
    && Array.isArray(b.add) && Array.isArray(b.overwrite) && Array.isArray(b.invalid)
    && b.invalid.every(e => !!e && typeof e === 'object' && typeof e.path === 'string')
}

// 长列表截断展示（覆盖确认弹窗用）：最多 max 行，超出追加「…等共 N 个」
function importListPreview(paths, max = 10) {
  const lines = paths.slice(0, max).map(p => `- ${p}`)
  if (paths.length > max) lines.push(`…等共 ${paths.length} 个`)
  return lines.join('\n')
}

// 导入完整流程（dry-run 报告 → 覆盖二次确认 → confirm 落盘）。抽成依赖注入的纯流程便于
// node 单测覆盖（vitest 无 vue 插件不能加载 Console.vue）。依赖：
//   importScripts(file, confirm, pkg)  api.importScripts
//   confirmDialog(msg) → bool          覆盖确认弹窗（Console 注入 window.confirm）
//   notify(msg, type)                  提示（Console 注入 toast）
//   refresh()                          导入成功后刷新列表
// 所有失败分支自行 notify 不向调用方抛错；返回 {ok, ...} 供测试断言。
export async function runPartitionImport({ file, pkg, importScripts, confirmDialog, notify, refresh }) {
  let dry
  try {
    dry = await importScripts(file, false, pkg)
  } catch (e) {
    notify('导入失败：' + e.message, 'error')
    return { ok: false }
  }
  const plan = summarizeImportReport(dry)
  if (!plan) {
    notify('导入失败：导入响应格式异常（端点契约可能已变更），未执行导入', 'error')
    return { ok: false }
  }
  if (plan.invalid.length) {
    const list = plan.invalid.slice(0, 8).map(e => `${e.path}（${e.reason}）`).join('；')
    const more = plan.invalid.length > 8 ? `…等共 ${plan.invalid.length} 个` : ''
    notify(`导入被阻止：${plan.invalid.length} 个文件未通过校验（服务端会整体拒绝导入），请修正压缩包后重试：${list}${more}`, 'error')
    return { ok: false }
  }
  if (plan.overwrite.length) {
    let msg = `导入到 ${pkg} 将覆盖 ${plan.overwrite.length} 个同名文件：\n${importListPreview(plan.overwrite)}`
    if (plan.add.length) msg += `\n\n另将新增 ${plan.add.length} 个文件：\n${importListPreview(plan.add)}`
    if (!confirmDialog(msg + '\n\n确认替换导入？')) return { ok: false, cancelled: true }
  }
  let rep
  try {
    rep = await importScripts(file, true, pkg)
  } catch (e) {
    // 服务端 confirm 遇 invalid 整体拒绝（400）等：error 字段即结构化原因
    notify('导入失败：' + e.message, 'error')
    return { ok: false }
  }
  const done = summarizeImportReport(rep)
  if (!done) {
    try { await refresh() } catch (e) { /* 刷新失败不吞导入结果提示 */ }
    notify('导入已执行，但结果报告无法解析（端点契约可能已变更）', 'warn')
    return { ok: true, parsed: false }
  }
  try {
    await refresh()
  } catch (e) {
    notify(`导入完成：新增 ${done.add.length} 个，覆盖 ${done.overwrite.length} 个；刷新列表失败：${e.message}`, 'warn')
    return { ok: true, add: done.add.length, overwrite: done.overwrite.length }
  }
  notify(`导入完成：新增 ${done.add.length} 个，覆盖 ${done.overwrite.length} 个`, 'success')
  return { ok: true, add: done.add.length, overwrite: done.overwrite.length }
}
