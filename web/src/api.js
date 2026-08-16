// 后端 API 封装（Rust 服务端）
const BASE = ''

async function req(method, path, body) {
  const opt = { method, headers: {} }
  if (body !== undefined) {
    opt.headers['Content-Type'] = 'application/json'
    opt.body = typeof body === 'string' ? body : JSON.stringify(body)
  }
  const r = await fetch(BASE + path, opt)
  if (!r.ok) {
    let msg = `HTTP ${r.status}`
    try { const j = await r.json(); if (j.error) msg = j.error } catch (e) {}
    throw new Error(msg)
  }
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

  // 模板
  listTemplates: () => req('GET', '/api/templates'),
  uploadTemplate: (name, dataB64) => req('POST', '/api/templates', { name, data_b64: dataB64 }),
  deleteTemplate: (name) => req('DELETE', `/api/templates/${encodeURIComponent(name)}`),
  testTemplate: (name, deviceId, threshold, region) =>
    req('POST', `/api/templates/${encodeURIComponent(name)}/test`, { device_id: deviceId, threshold, region }),

  // 脚本
  listScripts: () => req('GET', '/api/scripts'),
  saveScript: (s) => req('POST', '/api/scripts', s),
  deleteScript: (id) => req('DELETE', `/api/scripts/${id}`),
  runScript: (id, deviceId) => req('POST', `/api/scripts/${id}/run`, { device_id: deviceId }),
  stopScript: (id) => req('POST', `/api/scripts/${id}/stop`),

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
