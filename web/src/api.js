// 当前后端 API 封装（Rust 服务端）。
// 所有受保护请求遇到 401 都交给 Cookie 会话层处理；资源与运行接口不保留旧契约降级。
//
// V3 Package 化：脚本/函数库/模板/按键映射统一走
// `/api/packages/:pkg/plugins/:plugin/resources[...]`（plan §12-§14，寻址 =
// (package_id, plugin_id, path) 三元组；目录语义归插件定义）；
// 运行统一走 POST /api/runs（runner_id + entrypoint + payload，ADR-12/13：执行
// 目标按 runner 分发）。本封装只提供 runner 无关的通用 run()；具体 runner 的
// 包装（如 YAML 自动化 runner）归扩展前端侧（gamer-yaml-runner.js 等），Core API 层
// 不认识任何 runner 注册 id。
import { handleUnauthorized } from './auth'
import {
  GAMER_YAML_PLUGIN_ID, KEYMAP_PLUGIN_ID,
  AUTOMATION_DIR, FUNCTION_DIR, TEMPLATE_DIR, KEYMAP_DIR,
} from './gamer-plugin-ids'

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

/** Package 插件资源 URL（plan §12-§14：寻址 = (package, plugin, path) 三元组） */
function pkgResUrl(packageId, plugin, path = '') {
  let url = `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}/plugins/${encodeURIComponent(plugin)}/resources`
  if (path) url += `/${encodeURIComponent(path)}`
  return url
}

/** 资源 id "<package-id>/<文件路径>" → [packageId, 文件路径]（首段 = Package id） */
function splitResourceId(id) {
  const s = requireId(id, '资源 id')
  const i = s.indexOf('/')
  if (i <= 0 || i === s.length - 1) {
    throw new ApiError({
      status: 0,
      code: 'invalid_argument',
      message: '资源 id 必须形如 <package-id>/<文件路径>',
      data: { field: 'id' },
    })
  }
  return [s.slice(0, i), s.slice(i + 1)]
}

/** 资源 id 的文件路径段 → 插件内完整路径（目录由插件语义补全） */
function pluginPath(dir, file) {
  return `${dir}/${requireId(file, '文件路径')}`
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

/** 创建即落盘（无版本门禁；目标已存在由服务端 409） */
function createBody(content) {
  return { content }
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

function extensionUploadOptions(options = {}) {
  const headers = { 'Content-Type': 'application/zip' }
  // 免签名安装（Phase 1 收尾）：来源仅作服务端记录，无 proof 头；权限确认仍是唯一安装门禁
  if (options.source === 'official') headers['X-Gamer-Extension-Source'] = 'official'
  if (options.permissionConfirmed === true) headers['X-Gamer-Permission-Confirm'] = '1'
  return { rawBody: true, headers }
}

export const api = {
  // 登录/会话/退出见 src/auth.js（阶段 2 Cookie 会话；本封装不持有认证端点）

  // 扩展生命周期与动态 UI contribution
  listExtensions: () => req('GET', '/api/extensions'),
  listExtensionUi: () => req('GET', '/api/extensions/ui'),
  // Phase 10 插件管理：归档始终以 application/zip 上传，服务端重新验证
  // 来源标注与权限确认（免签名，完整性行可选 x-expected-sha256）。
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
  // 安装本地 APK：raw 字节直传（服务端组限额 1GiB，临时文件后 adb install -r）；
  // filename 供服务端做扩展名校验与日志
  installApk: async (id, file) => {
    const r = await response(
      'POST',
      `/api/devices/${encodeURIComponent(requireId(id, 'device_id'))}/install-apk?filename=${encodeURIComponent(file.name)}`,
      file,
      { rawBody: true, headers: { 'Content-Type': 'application/vnd.android.package-archive' } },
    )
    return readResult(r)
  },

  // ---- 媒体库（视频工作台 V1，实施合同 §1；素材寻址一律 media id）----
  // 列表：{"media":[MediaMetadata]}（创建时间倒序）→ 解包数组
  listMedia: async () => {
    const rep = await req('GET', '/api/media')
    return Array.isArray(rep?.media) ? rep.media : []
  },
  getMedia: (id) => req('GET', `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}`),
  // 导入原始字节（application/octet-stream，服务端组限额 1GiB）→ 201 MediaMetadata
  importMedia: async (bytes, name) => {
    const r = await response(
      'POST',
      `/api/media/import?name=${encodeURIComponent(requireId(name, 'name'))}`,
      bytes,
      { rawBody: true, headers: { 'Content-Type': 'application/octet-stream' } },
    )
    return readResult(r)
  },
  // 删除素材（仍被引用 → 409 {"error":"media_referenced"}）
  deleteMedia: (id) => req('DELETE', `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}`),
  // 播放 / 精确帧 URL（只拼 URL 不发请求；<video> 播放用 file，确定帧用 frame PNG）
  mediaFileUrl: (id) => `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/file`,
  // frame 查询参数 = pts_us|index|max_width（pts_us 与 index 同时给出时 pts_us 优先；
  // 微秒取整，非法值归 0；同一请求服务端逐字节可重复）
  mediaFrameUrl: (id, { ptsUs, index, maxWidth } = {}) => {
    const p = new URLSearchParams()
    if (ptsUs !== undefined && ptsUs !== null) p.set('pts_us', String(Math.max(0, Math.round(Number(ptsUs) || 0))))
    if (index !== undefined && index !== null) p.set('index', String(Math.max(0, Math.round(Number(index) || 0))))
    if (maxWidth !== undefined && maxWidth !== null) p.set('max_width', String(Math.max(1, Math.round(Number(maxWidth) || 1))))
    const q = p.toString()
    const base = `/api/media/${encodeURIComponent(requireId(id, 'media_id'))}/frame`
    return q ? `${base}?${q}` : base
  },

  // ---- 录制（实施合同 §2；服务端权威：浏览器断开不中断，stop/cancel 幂等）----
  recordingStart: (deviceId) => req('POST', '/api/recording/start', { device_id: requireId(deviceId, 'device_id') }),
  recordingStop: (id) => req('POST', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/stop`),
  recordingCancel: (id) => req('POST', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/cancel`),
  recordingStatus: (id) => req('GET', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}`),
  // 设备当前活动会话：404（无活动会话，轮询常态）→ null 不抛错
  activeRecording: async (deviceId) => {
    try {
      return await req('GET', `/api/recording/active?device_id=${encodeURIComponent(requireId(deviceId, 'device_id'))}`)
    } catch (e) {
      if (e?.status === 404) return null
      throw e
    }
  },
  // 操作事件：{schema_version, events:[InputEventRecord]}（时间轴升序）→ 解包数组
  recordingEvents: async (id) => {
    const rep = await req('GET', `/api/recording/${encodeURIComponent(requireId(id, 'recording_id'))}/events`)
    return Array.isArray(rep?.events) ? rep.events : []
  },
  // YAML v3 草稿生成：经现有扩展 call 通路（扩展 id 字面量唯一归宿 =
  // gamer-plugin-ids.js，本文件不出现 id 字面量）。返回 {yaml, diagnostics}；
  // 兼容 {ok, data} 信封形态。
  createVideoDraft: async (recordingId, eventIds) => {
    const rep = await req(
      'POST',
      `/api/extensions/${encodeURIComponent(GAMER_YAML_PLUGIN_ID)}/call`,
      {
        action: 'automation.create_draft',
        values: {
          recording_id: requireId(recordingId, 'recording_id'),
          event_ids: Array.isArray(eventIds) ? eventIds : [],
        },
      },
    )
    return rep && typeof rep === 'object' && rep.data && typeof rep.data === 'object' ? rep.data : rep
  },

  // ---- Package（plan §7-§11：Package = 配置数据上下文，独立 Package ID）----
  listPackages: () => req('GET', '/api/packages'),
  createPackage: (p) => req('POST', '/api/packages', p),
  getPackage: (packageId) => req('GET', `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}`),
  // 元数据编辑（部分字段；条件更新 expected_revision 或 force:true）
  updatePackage: async (packageId, patch = {}) => req('PUT', `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}`, patch),
  deletePackage: (packageId) => req('DELETE', `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}`),
  duplicatePackage: (packageId, newId) => req(
    'POST',
    `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}/duplicate`,
    { new_id: requireId(newId, 'new_id') },
  ),
  // targets 兼容性（plan §17，warning 语义：不兼容仅提示不禁止）
  packageCompatibility: (packageId, androidPackage) => req(
    'GET',
    `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}/compatibility?android_package=${encodeURIComponent(requireId(androidPackage, 'android_package'))}`,
  ),
  // 导入 .gamerpkg 原始字节（Content-Type: application/zip）；expectedSha256 →
  // X-Expected-Sha256；overwrite=true 原子替换已存在包（409 = 已存在且未确认覆盖）
  importPackageArchive: async (bytes, { expectedSha256, overwrite = false } = {}) => {
    const headers = { 'Content-Type': 'application/zip' }
    if (expectedSha256) headers['X-Expected-Sha256'] = String(expectedSha256)
    const r = await response(
      'POST', `/api/packages/import${overwrite ? '?overwrite=true' : ''}`, bytes,
      { rawBody: true, headers },
    )
    return readResult(r)
  },
  // 导出当前 Package 为 .gamerpkg：200 二进制 + Content-Disposition 文件名 + X-Content-Sha256。
  // includeMedia=true → ?include_media=true（归档追加 media/files/<sha256> 素材字节；
  // 默认只带 media/index.json 引用登记，不含原始大视频）
  exportPackageArchive: async (packageId, { includeMedia = false } = {}) => {
    const r = await req(
      'POST',
      `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}/export${includeMedia ? '?include_media=true' : ''}`,
      {},
    )
    const blob = await r.blob()
    return {
      blob,
      filename: filenameFromDisposition(r.headers.get('content-disposition')),
      sha256: r.headers.get('x-content-sha256') || '',
    }
  },

  // ---- 插件资源通用原语（plan §12-§14；文本 JSON / 字节原始流）----
  listPluginResources: (packageId, plugin, prefix = '') =>
    req('GET', pkgResUrl(packageId, plugin) + (prefix ? `?prefix=${encodeURIComponent(prefix)}` : '')),
  getPluginResource: (packageId, plugin, path) => req('GET', pkgResUrl(packageId, plugin, path)),
  putPluginResourceText: (packageId, plugin, path, { content, expected_version, force } = {}) =>
    req('PUT', pkgResUrl(packageId, plugin, path), updateBody({ content, expected_version, force }, path)),
  putPluginResourceBytes: async (packageId, plugin, path, bytes, { expectedVersion, force = false } = {}) => {
    const headers = { 'Content-Type': 'application/octet-stream' }
    if (expectedVersion) headers['X-Expected-Version'] = String(expectedVersion)
    if (force) headers['X-Force'] = 'true'
    const r = await response('PUT', pkgResUrl(packageId, plugin, path), bytes, { rawBody: true, headers })
    return readResult(r)
  },
  deletePluginResource: (packageId, plugin, path) => req('DELETE', pkgResUrl(packageId, plugin, path)),
  renamePluginResource: (packageId, plugin, path, newPath) => req(
    'POST',
    `/api/packages/${encodeURIComponent(requireId(packageId, 'package_id'))}/plugins/${encodeURIComponent(plugin)}/rename`,
    { path: requireId(path, 'path'), new_path: requireId(newPath, 'new_path') },
  ),

  // ---- 扩展资源兼容封装（id 字面量唯一归宿 = gamer-plugin-ids.js；资源 id 契约
  // "<package-id>/<文件路径>" 不变，仅首段语义从 Android 包名切换为 Package id，
  // 目录段由插件语义补全；runner 包装仍归 gamer-yaml-runner.js）----
  // 按键映射（映射插件 mappings/ 目录）
  listKeymaps: async (packageId) => {
    const rep = await api.listPluginResources(packageId, KEYMAP_PLUGIN_ID, KEYMAP_DIR)
    return (rep?.resources || []).map(r => {
      const file = r.path.slice(KEYMAP_DIR.length + 1)
      return { id: `${r.package}/${file}`, name: file, file, version: r.version, package: r.package }
    })
  },
  getKeymap: async (name, packageId) => {
    const [, file] = splitResourceId(keymapId(name, packageId))
    const entry = await api.getPluginResource(packageId, KEYMAP_PLUGIN_ID, pluginPath(KEYMAP_DIR, file))
    return { ...entry, name: file, file }
  },
  createKeymap: ({ pkg, name, content } = {}) =>
    req('PUT', pkgResUrl(pkg, KEYMAP_PLUGIN_ID, pluginPath(KEYMAP_DIR, requireId(name, 'name'))), createBody(content)),
  updateKeymap: async (name, pkg, payload = {}) => {
    const [, file] = splitResourceId(keymapId(name, pkg))
    return api.putPluginResourceText(pkg, KEYMAP_PLUGIN_ID, pluginPath(KEYMAP_DIR, file), payload)
  },
  deleteKeymap: (name, pkg) => {
    const [, file] = splitResourceId(keymapId(name, pkg))
    return api.deletePluginResource(pkg, KEYMAP_PLUGIN_ID, pluginPath(KEYMAP_DIR, file))
  },

  // 模板（自动化插件 templates/ 目录；上传字节经服务端钩子做 8-bit 灰度归一化）
  listTemplates: async (packageId) => {
    const rep = await api.listPluginResources(packageId, GAMER_YAML_PLUGIN_ID, TEMPLATE_DIR)
    return (rep?.resources || []).map(r => ({
      name: r.path.slice(TEMPLATE_DIR.length + 1),
      pkg: r.package,
      version: r.version,
      updated_at: r.updated_at,
      size: r.size,
    }))
  },
  // 客户端组合完整文件名（短名 + #区域后缀 + #1 颜色标记），原始字节上传；
  // 灰度归一化由服务端字节钩子统一执行。
  createTemplate: async (shortName, dataB64, packageId, region, preserveColor = false) => {
    const name = composeTemplateName(shortName, region, preserveColor)
    return api.putPluginResourceBytes(
      requireId(packageId, 'package_id'), GAMER_YAML_PLUGIN_ID,
      pluginPath(TEMPLATE_DIR, name), base64ToBytes(dataB64), { force: true },
    )
  },
  // 图片替换：模板名来自调用方，body 只有原始图片字节。
  replaceTemplateImage: async (name, dataB64, packageId) =>
    api.putPluginResourceBytes(
      requireId(packageId, 'package_id'), GAMER_YAML_PLUGIN_ID,
      pluginPath(TEMPLATE_DIR, requireId(name, 'name')), base64ToBytes(dataB64), { force: true },
    ),
  // 批量导入上传原始字节建模板（名字已由调用方按模板名规则清洗），
  // 灰度归一化仍由服务端字节钩子统一执行。
  importTemplateBytes: (name, bytes, packageId) =>
    api.putPluginResourceBytes(
      requireId(packageId, 'package_id'), GAMER_YAML_PLUGIN_ID,
      pluginPath(TEMPLATE_DIR, requireId(name, 'name')), bytes, { force: true },
    ),
  // 重命名：服务端经扩展 before_rename 钩子同步改写脚本/函数引用（v3 AST）。
  renameTemplate: (oldName, newName, packageId) =>
    api.renamePluginResource(
      packageId, GAMER_YAML_PLUGIN_ID,
      pluginPath(TEMPLATE_DIR, requireId(oldName, 'name')),
      pluginPath(TEMPLATE_DIR, requireId(newName, 'new_name')),
    ),
  deleteTemplate: (name, packageId) =>
    api.deletePluginResource(packageId, GAMER_YAML_PLUGIN_ID, pluginPath(TEMPLATE_DIR, requireId(name, 'name'))),
  // 模板匹配测试 = vision 能力位语义（pkg = Package id，plugin 必填）
  testTemplate: (name, deviceId, threshold, region, packageId) =>
    req('POST', '/api/capabilities/vision/test', {
      device_id: deviceId, threshold, region,
      pkg: requireId(packageId, 'package_id'), plugin: GAMER_YAML_PLUGIN_ID, name,
    }),
  // 模板缩略图/预览 URL（<img :src> 用；GET 二进制返回原始 PNG）
  tplImageUrl: (name, packageId) => pkgResUrl(requireId(packageId, 'package_id'), GAMER_YAML_PLUGIN_ID, pluginPath(TEMPLATE_DIR, name)),

  // 脚本（自动化插件 automations/ 目录；id 形如 "<package-id>/<name>.yaml"，含 '/'，
  // 拼 URL 必须整体 encodeURIComponent）
  listScripts: async (packageId) => {
    const rep = await api.listPluginResources(packageId, GAMER_YAML_PLUGIN_ID, AUTOMATION_DIR)
    return (rep?.resources || []).map(r => ({
      id: `${r.package}/${r.path.slice(AUTOMATION_DIR.length + 1)}`,
      package: r.package,
      name: r.path.slice(AUTOMATION_DIR.length + 1),
      version: r.version,
      updated_at: r.updated_at,
      size: r.size,
    }))
  },
  // 单脚本读取（含内容版本短码 version：编辑器 expected_version 冲突检测依据）
  getScript: (id) => {
    const [pkg, file] = splitResourceId(id)
    return api.getPluginResource(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(AUTOMATION_DIR, file))
  },
  // PUT 创建或更新（创建冲突 409；更新缺版本时在客户端拒绝，force 必须显式为 true）
  createScript: ({ name, content, pkg } = {}) =>
    req('PUT', pkgResUrl(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(AUTOMATION_DIR, requireId(name, 'name'))), createBody(content)),
  updateScript: async (id, payload = {}) => {
    const [pkg, file] = splitResourceId(id)
    return api.putPluginResourceText(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(AUTOMATION_DIR, file), payload)
  },
  deleteScript: (id) => {
    const [pkg, file] = splitResourceId(id)
    return api.deletePluginResource(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(AUTOMATION_DIR, file))
  },
  // 函数库（自动化插件 functions/ 目录；id 形如 "<package-id>/<文件短路径>.yaml"，
  // 文件短路径可含目录。不进脚本列表/运行接口/任务选择器；GET 单文件含
  // content/version/functions（顶层函数名清单，扩展注记提供））
  listFunctions: async (packageId) => {
    const rep = await api.listPluginResources(packageId, GAMER_YAML_PLUGIN_ID, FUNCTION_DIR)
    return (rep?.resources || []).map(r => {
      const file = r.path.slice(FUNCTION_DIR.length + 1)
      return {
        id: `${r.package}/${file}`, pkg: r.package, file,
        content: r.content, version: r.version, functions: r.meta?.functions || [],
        updated_at: r.updated_at,
      }
    })
  },
  getFunction: (id) => {
    const [pkg, file] = splitResourceId(id)
    return api.getPluginResource(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(FUNCTION_DIR, file))
  },
  // PUT 创建或更新/重命名，更新缺版本时在客户端拒绝。
  createFunction: ({ pkg, name, content } = {}) =>
    req('PUT', pkgResUrl(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(FUNCTION_DIR, requireId(name, 'name'))), createBody(content)),
  updateFunction: async (id, payload = {}) => {
    const [pkg, file] = splitResourceId(id)
    return api.putPluginResourceText(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(FUNCTION_DIR, file), payload)
  },
  deleteFunction: (id) => {
    const [pkg, file] = splitResourceId(id)
    return api.deletePluginResource(pkg, GAMER_YAML_PLUGIN_ID, pluginPath(FUNCTION_DIR, file))
  },

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
  // 参数 schema descriptor（P12.3 / 契约 §7）：前端不为取参数而解析 YAML——按
  // runner + entrypoint 资源 id（"<pkg>/<脚本>.yaml" 或 "<pkg>/<文件>.yaml#<函数>"，
  // 含 '/'/'#'，整体 encodeURIComponent）查询可渲染表单的参数 schema。
  // 200 = {runner_id, entrypoint, kind, format, schema, signature}（schema→表单
  // 声明的适配见 script-editor/entrypointParams.ts；signature 为 psig1 参数签名，
  // 本期仅透传）。结构化错误原样上抛（ApiError.status/code/data）：
  // 404 {error:"runner_not_found"} / 404 {error:"not_found",resource} /
  // 400 {error:"invalid_script",diagnostics:[...]}
  getEntrypointParams: (runnerId, entrypoint) => req(
    'GET',
    `/api/runners/${encodeURIComponent(requireId(runnerId, 'runner_id'))}/entrypoint?entrypoint=${encodeURIComponent(requireId(entrypoint, 'entrypoint'))}`,
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
