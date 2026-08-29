// 后端 API 封装（Rust 服务端）
// 全站鉴权拦截（阶段 2）：除登录/探测/退出三个豁免端点外，任何响应 401 →
// 清本地缓存态并跳 #/login（保留回跳参数），各视图不改调用方式。
import { handleUnauthorized } from './auth'

const BASE = ''

// 认证三端点自身管理 401 语义，不走全站拦截（否则登录失败会被误跳转/死循环）
function authExempt(url) {
  return url.startsWith('/api/login') || url.startsWith('/api/session') || url.startsWith('/api/logout')
}

// 错误响应 → 结构化 Error：保留 message（body.error 或 HTTP xxx，与旧文案一致），
// 并附加 status / data 原始 JSON —— 调用方据此识别 409 设备冲突、404 端点缺失（旧后端降级）等
async function errMsg(r) {
  let body = null
  try { body = await r.json() } catch (e) {}
  const err = new Error(body && body.error ? body.error : `HTTP ${r.status}`)
  err.status = r.status
  err.data = body
  return err
}

async function readResult(r) {
  const ct = r.headers.get('content-type') || ''
  if (ct.includes('application/json')) return r.json()
  return r
}

async function req(method, path, body) {
  const opt = { method, headers: {} }
  if (body !== undefined) {
    opt.headers['Content-Type'] = 'application/json'
    opt.body = typeof body === 'string' ? body : JSON.stringify(body)
  }
  const r = await fetch(BASE + path, opt)
  if (!r.ok) {
    if (r.status === 401 && !authExempt(path)) {
      handleUnauthorized()                    // 会话过期/未认证：清态 + 跳登录
      throw await errMsg(r)                   // 调用方 catch 里仍能拿到原因（含 status/data）
    }
    throw await errMsg(r)
  }
  return readResult(r)
}

export const api = {
  // 登录/会话/退出见 src/auth.js（阶段 2 Cookie 会话；本封装不持有认证端点）

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

  // 模板（按应用分区 data/<pkg>/tmpl；pkg 缺省=跨分区全列）
  listTemplates: (pkg) => req('GET', `/api/templates${pkg ? `?pkg=${encodeURIComponent(pkg)}` : ''}`),
  uploadTemplate: (name, dataB64, pkg) => req('POST', '/api/templates', { name, data_b64: dataB64, pkg }),
  // 录制上传（阶段 6）：短名 + 搜索区域（相对坐标 [x1,y1,x2,y2]，0~1）→
  // 完整文件名 `短名#x1_y1_x2_y2.png`（×1000 三位整数，与 defaultTemplateName 同编码）。
  // 现服务端 POST /api/templates 只收完整 name（validate_template_name 保留 # 后缀），
  // 尚无「短名+region」参数形态，#元数据暂由前端拼接；服务端补齐参数后此处整体切换（见汇报）。
  uploadTemplateRegion: (shortName, dataB64, pkg, region) =>
    req('POST', '/api/templates', { name: composeRegionName(shortName, region), data_b64: dataB64, pkg }),
  renameTemplate: (oldName, newName, pkg) =>
    req('PUT', `/api/templates/${encodeURIComponent(oldName)}?pkg=${encodeURIComponent(pkg)}`, { name: newName }),
  deleteTemplate: (name, pkg) =>
    req('DELETE', `/api/templates/${encodeURIComponent(name)}?pkg=${encodeURIComponent(pkg)}`),
  testTemplate: (name, deviceId, threshold, region, pkg) =>
    req('POST', `/api/templates/${encodeURIComponent(name)}/test`, { device_id: deviceId, threshold, region, pkg }),
  // 模板缩略图/预览 URL（<img :src> 用；pkg 必填）
  tplImageUrl: (name, pkg) => `/api/templates/${encodeURIComponent(name)}/image?pkg=${encodeURIComponent(pkg)}`,

  // 脚本（id 形如 "<pkg>/<name>.yaml"，含 '/'，拼 URL 必须整体 encodeURIComponent；保存需 pkg=应用分区）
  listScripts: () => req('GET', '/api/scripts'),
  // 单脚本读取（含内容版本短码 version：编辑器 expected_version 冲突检测依据）
  getScript: (id) => req('GET', `/api/scripts/${encodeURIComponent(id)}`),
  // 保存（upsert；id 缺省=新建，id+新名=重命名并删旧文件）。expected_version 与磁盘不符
  // → 409 {code:"version_conflict", message, resource}（err.status/err.data 可取）
  saveScript: (s) => req('POST', '/api/scripts', s),
  deleteScript: (id) => req('DELETE', `/api/scripts/${encodeURIComponent(id)}`),
  // 函数库（data/<pkg>/func/；id 形如 "<pkg>/<文件短路径>.yaml"，整体 encodeURIComponent。
  // 不进脚本列表/运行接口/任务选择器；GET 单文件含 content/version/functions（顶层函数名清单））
  listFunctions: (pkg) => req('GET', `/api/functions?pkg=${encodeURIComponent(pkg)}`),
  getFunction: (id) => req('GET', `/api/functions/${encodeURIComponent(id)}`),
  // 创建/覆盖（upsert）：{pkg, name(短路径,缺扩展名自动补), content, expected_version?}
  saveFunction: (f) => req('POST', '/api/functions', f),
  // 覆盖更新（不重命名）：{content, expected_version?}；404（不存在）优先于 409
  updateFunction: (id, f) => req('PUT', `/api/functions/${encodeURIComponent(id)}`, f),
  deleteFunction: (id) => req('DELETE', `/api/functions/${encodeURIComponent(id)}`),

  // 脚本运行（阶段 5 契约）：body {device_id, start_index?, args?}——args 为稀疏显式覆盖映射
  //（bool/coord/time/color/tmpl/key/text 七类；「使用默认值」的参数省略，由服务端解析默认值）。
  // 成功 202 {run_id, state, resolved_args}；参数诊断 400 {error:"invalid_args", diagnostics:[...]}
  //（err.status/err.data 可取）；设备占用 409 {error:"device_busy", run_id, script_id, source, started_at}
  runScript: (id, deviceId, startIndex, args) =>
    req('POST', `/api/scripts/${encodeURIComponent(id)}/run`, {
      device_id: deviceId,
      start_index: startIndex || 0,
      ...(args && Object.keys(args).length ? { args } : {}),
    }),
  // 函数测试（阶段 5）：id = 函数库文件 id（"<pkg>/<文件短路径>.yaml"，整体 encodeURIComponent）。
  // body {device_id, function?, start_index?, args?}（function 缺省 = 文件第一个函数）；
  // 响应/错误语义与脚本 run 相同（RunManager 统一 run_id 管理）
  runFunction: (id, deviceId, opts = {}) =>
    req('POST', `/api/functions/${encodeURIComponent(id)}/run`, {
      device_id: deviceId,
      ...(opts.function ? { function: opts.function } : {}),
      ...(opts.start_index ? { start_index: opts.start_index } : {}),
      ...(opts.args && Object.keys(opts.args).length ? { args: opts.args } : {}),
    }),
  stopScript: (id) => req('POST', `/api/scripts/${encodeURIComponent(id)}/stop`),
  scriptStatus: (id) => req('GET', `/api/scripts/${encodeURIComponent(id)}/status`),
  // 统一运行实例（run_id 主键）：单次查询 RunRecord / 按次取消（终态以查询为准）
  getRun: (runId) => req('GET', `/api/runs/${encodeURIComponent(runId)}`),
  cancelRun: (runId) => req('POST', `/api/runs/${encodeURIComponent(runId)}/cancel`),
  // 设备当前运行中的脚本（页面刷新后恢复运行态用）
  // 新契约 → {active:true,...RunRecord} | {active:false}；旧后端 → {running, script_id?, script_name?}
  deviceRun: (id) => req('GET', `/api/devices/${id}/run`),
  // 导出整分区快照 zip（yaml/ + tmpl/ 全量，?pkg= 指定分区）→ { blob, filename }
  exportPartition: async (pkg) => {
    const r = await fetch(`/api/scripts/export?pkg=${encodeURIComponent(pkg)}`)
    if (!r.ok) {
      if (r.status === 401) handleUnauthorized()
      throw await errMsg(r)
    }
    const cd = r.headers.get('content-disposition') || ''
    let filename = ''
    const m = cd.match(/filename\*=UTF-8''([^;\s]+)/) || cd.match(/filename="?([^";\s]+)"?/)
    if (m) { try { filename = decodeURIComponent(m[1]) } catch (e) { filename = m[1] } }
    return { blob: await r.blob(), filename }
  },
  // 导入分区快照 zip 到指定应用分区：confirm=false dry-run 只解析报告，true 落盘（同名替换）
  importScripts: async (file, confirm, pkg) => {
    const r = await fetch(`/api/scripts/import?confirm=${confirm ? 1 : 0}&pkg=${encodeURIComponent(pkg)}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/zip' },
      body: file
    })
    if (!r.ok) {
      if (r.status === 401) handleUnauthorized()
      throw await errMsg(r)
    }
    return r.json()
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
  // 任务立即执行（用任务已存参数快照；过期/无快照由服务端明确报错）：
  // 新契约 202 {run_id}（触发即返回，不等任务完成）；旧后端 200 {ok:true}
  runTaskNow: (id) => req('POST', `/api/tasks/${id}/run`),

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

// ---- 模板搜索区域命名（录制上传用）----
// 与 console/geometry.js defaultTemplateName 同一编码：相对坐标 ×1000 存 3 位整数
//（0.123→'123'，1.0 夹取 999），无后缀文件名回退全屏（=#a 语义）。
export function composeRegionName(shortName, region) {
  const base = String(shortName || '').replace(/\.(png|jpe?g)$/i, '')
  if (!Array.isArray(region) || region.length !== 4 || region.some(v => !Number.isFinite(v))) {
    return `${base}.png`
  }
  const toInt3 = v => String(Math.min(999, Math.max(0, Math.round(v * 1000)))).padStart(3, '0')
  const suffix = `${toInt3(region[0])}_${toInt3(region[1])}_${toInt3(region[2])}_${toInt3(region[3])}`
  return `${base}#${suffix}.png`
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
