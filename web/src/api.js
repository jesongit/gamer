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
  renameTemplate: (oldName, newName, pkg) =>
    req('PUT', `/api/templates/${encodeURIComponent(oldName)}?pkg=${encodeURIComponent(pkg)}`, { name: newName }),
  deleteTemplate: (name, pkg) =>
    req('DELETE', `/api/templates/${encodeURIComponent(name)}?pkg=${encodeURIComponent(pkg)}`),
  testTemplate: (name, deviceId, threshold, region, pkg) =>
    req('POST', `/api/templates/${encodeURIComponent(name)}/test`, { device_id: deviceId, threshold, region, pkg }),
  // 模板缩略图/预览 URL（<img :src> 用；pkg 必填）
  tplImageUrl: (name, pkg) => `/api/templates/${encodeURIComponent(name)}/image?pkg=${encodeURIComponent(pkg)}`,

  // 配置：操作记录 YAML 模板（config.toml [op_templates]）
  getOpTemplates: () => req('GET', '/api/op-templates'),

  // 脚本（id 形如 "<pkg>/<name>.yaml"，含 '/'，拼 URL 必须整体 encodeURIComponent；保存需 pkg=应用分区）
  listScripts: () => req('GET', '/api/scripts'),
  saveScript: (s) => req('POST', '/api/scripts', s),
  deleteScript: (id) => req('DELETE', `/api/scripts/${encodeURIComponent(id)}`),
  // 脚本运行（RUN-003 阶段3 契约）：成功 202 {run_id, state:"starting"}；设备占用 409
  // {error:"device_busy", run_id, script_id, source, started_at}（err.status/err.data 可取）
  runScript: (id, deviceId, startIndex, func) => req('POST', `/api/scripts/${encodeURIComponent(id)}/run`, { device_id: deviceId, start_index: startIndex || 0, ...(func ? { func } : {}) }),
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
  // 导入分区快照 zip 到指定应用分区：confirm=false 只探测冲突，true 落盘（同名替换）
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

  // 定时任务
  listTasks: () => req('GET', '/api/tasks'),
  saveTask: (t) => req('POST', '/api/tasks', t),
  deleteTask: (id) => req('DELETE', `/api/tasks/${id}`),
  // 任务立即执行：新契约 202 {run_id}（触发即返回，不等任务完成）；旧后端 200 {ok:true}
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
