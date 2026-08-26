// 后端 API 封装（Rust 服务端）
const BASE = ''

async function errMsg(r) {
  let msg = `HTTP ${r.status}`
  try { const j = await r.json(); if (j.error) msg = j.error } catch (e) {}
  return msg
}

async function req(method, path, body) {
  const opt = { method, headers: {} }
  if (body !== undefined) {
    opt.headers['Content-Type'] = 'application/json'
    opt.body = typeof body === 'string' ? body : JSON.stringify(body)
  }
  const r = await fetch(BASE + path, opt)
  if (!r.ok) throw new Error(await errMsg(r))
  const ct = r.headers.get('content-type') || ''
  if (ct.includes('application/json')) return r.json()
  return r
}

export const api = {
  // 认证
  login: (user, password) => req('POST', '/api/login', { user, password }),

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
  runScript: (id, deviceId, startIndex, func) => req('POST', `/api/scripts/${encodeURIComponent(id)}/run`, { device_id: deviceId, start_index: startIndex || 0, ...(func ? { func } : {}) }),
  stopScript: (id) => req('POST', `/api/scripts/${encodeURIComponent(id)}/stop`),
  scriptStatus: (id) => req('GET', `/api/scripts/${encodeURIComponent(id)}/status`),
  // 设备当前运行中的脚本（页面刷新后恢复运行态用）→ {running, script_id?, script_name?}
  deviceRun: (id) => req('GET', `/api/devices/${id}/run`),
  // 导出整分区快照 zip（yaml/ + tmpl/ 全量，?pkg= 指定分区）→ { blob, filename }
  exportPartition: async (pkg) => {
    const r = await fetch(`/api/scripts/export?pkg=${encodeURIComponent(pkg)}`)
    if (!r.ok) throw new Error(await errMsg(r))
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
    if (!r.ok) throw new Error(await errMsg(r))
    return r.json()
  },

  // 定时任务
  listTasks: () => req('GET', '/api/tasks'),
  saveTask: (t) => req('POST', '/api/tasks', t),
  deleteTask: (id) => req('DELETE', `/api/tasks/${id}`),
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
