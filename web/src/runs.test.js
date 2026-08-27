// RUN-003 前端状态迁移：运行实例以 run_id 为主键的语义工具、注册表状态机与 API 契约用例。
// 全部 stub-fetch（node 环境），覆盖五组场景：
//   ① 409 设备冲突格式化（来源标签/本地化时间）
//   ② GET /api/devices/:id/run 的 active:false 与新旧形状恢复路径
//   ③ 启动 202 快速返回流程（启动即存 runId，不阻塞等待完成）
//   ④ cancel 后状态机迁移（stopping 先行 → 终态归档复位 / 迟到刷新拒收）
//   ⑤ started_at ISO → 本地时区展示
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

function lsStub(initial = {}) {
  const m = new Map(Object.entries(initial))
  return {
    getItem: k => (m.has(k) ? m.get(k) : null),
    setItem: (k, v) => { m.set(k, String(v)) },
    removeItem: k => { m.delete(k) },
    clear: () => { m.clear() },
    _m: m,
  }
}

/** 构造 Response 形状 stub（api.js 只消费 ok/status/headers/json/blob） */
function res(status, body, ct = 'application/json') {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: h => (/^content-type$/i.test(h) ? ct : null) },
    json: async () => body,
  }
}

let runs, storeMod, api

/** 每个用例独立模块实例：resetModules 后动态导入（runRegistry 是模块级单例，需隔离） */
async function fresh() {
  vi.resetModules()
  ;({ ...runs } = await import('../src/runs'))
  storeMod = await import('../src/store')
  ;({ api } = await import('../src/api'))
}

beforeEach(async () => {
  vi.stubGlobal('fetch', vi.fn())
  vi.stubGlobal('localStorage', lsStub())
  vi.stubGlobal('location', { hash: '#/console' })
  await fresh()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// ---------- ① 409 设备冲突格式化 ----------
describe('① 409 冲突：来源标签与冲突文案格式化', () => {
  it('sourceLabel 映射契约三种来源，未知值原样透出，空值为空串', () => {
    expect(runs.sourceLabel('manual')).toBe('手动')
    expect(runs.sourceLabel('scheduled')).toBe('定时')
    expect(runs.sourceLabel('task_now')).toBe('手动任务')
    expect(runs.sourceLabel('other')).toBe('other')
    expect(runs.sourceLabel('')).toBe('')
    expect(runs.sourceLabel(null)).toBe('')
  })

  it('describeConflict 完整字段 → 文案含对方脚本、来源中文标签、本地化开始时间；缺槽位回退"未知"', () => {
    const busy = { error: 'device_busy', run_id: 'r-1', script_id: 'hkrpg/daily.yaml', source: 'scheduled', started_at: '2026-08-27T07:30:05Z' }
    const msg = runs.describeConflict(busy)
    expect(msg).toContain('hkrpg/daily.yaml')
    expect(msg).toContain('来源：定时')
    expect(msg).toContain(`开始于 ${runs.formatLocalTime(busy.started_at)}`)
    const bare = runs.describeConflict({ error: 'device_busy' })
    expect(bare).toContain('未知脚本')
    expect(bare).toContain('未知来源')
    expect(bare).toContain('未知时间')
  })

  it('api 层把 409 转结构化错误：err.status/err.data 可取且 message 沿用旧文案口径', async () => {
    fetch.mockResolvedValueOnce(res(409, { error: 'device_busy', run_id: 'r-9', script_id: 'p/s.yml', source: 'manual', started_at: '2026-08-27T00:00:00Z' }))
    let err = null
    await api.runScript('p/s.yml', 'dev-1').catch(e => { err = e })
    expect(err).not.toBeNull()
    expect(err.status).toBe(409)
    expect(err.data.error).toBe('device_busy')
    expect(err.data.script_id).toBe('p/s.yml')
    expect(err.message).toBe('device_busy')
    expect(runs.isDeviceBusyConflict(err)).toBe(true)
  })
})

// ---------- ② active:false 恢复路径 ----------
describe('② 设备当前 run 响应归一化与恢复路径', () => {
  it('无活动响应归一化为 null（新契约 active:false / 旧后端 running:false / 脏输入）', () => {
    expect(runs.normalizeActiveRunResponse({ active: false })).toBeNull()
    expect(runs.normalizeActiveRunResponse({ running: false })).toBeNull()
    expect(runs.normalizeActiveRunResponse(null)).toBeNull()
    expect(runs.normalizeActiveRunResponse({})).toBeNull()
  })

  it('新契约 active:true → RunRecord 兼容对象；旧后端 {running,script_id} → 降级形状（无 run_id）', () => {
    const rec = runs.normalizeActiveRunResponse({
      active: true, run_id: 'rid-1', device_id: 'dev-1', script_id: 'hkrpg/login.yml',
      state: 'stopping', source: 'task_now', task_id: 't1', scheduled_at: null,
      started_at: '2026-08-27T01:02:03Z', finished_at: null, error: null,
    })
    expect(rec).toMatchObject({
      run_id: 'rid-1', device_id: 'dev-1', script_id: 'hkrpg/login.yml',
      state: 'stopping', source: 'task_now',
    })
    // 非法 state 兜底为 running（保守按活动处理，由下一次查询纠正）
    expect(runs.normalizeActiveRunResponse({ active: true, run_id: 'x', state: '?', script_id: 'a' }).state).toBe('running')

    const legacy = runs.normalizeActiveRunResponse({ running: true, script_id: 'p/old.yml', script_name: '旧脚本' })
    expect(legacy).toMatchObject({ run_id: null, script_id: 'p/old.yml', script_name: '旧脚本', state: 'running', legacy: true })
  })

  it('恢复路径：normalize 后登记注册表 → 反查命中、全局态点亮、display 不被裸 script_id 覆盖', async () => {
    const rep = { active: true, run_id: 'rid-2', device_id: 'dev-2', script_id: 'p/b.yml', state: 'running', source: 'scheduled' }
    const rec = runs.normalizeActiveRunResponse(rep)
    storeMod.store.deviceId = 'dev-2'
    const m = storeMod.applyRunRecord({ ...rec, display: '日常登录（定时）' })
    expect(m.display).toBe('日常登录（定时）')
    expect(storeMod.getActiveRun('dev-2')?.run_id).toBe('rid-2')
    expect(storeMod.findRun('rid-2')).toBeTruthy()
    expect(storeMod.store.running).toBe(true)
    expect(storeMod.store.runId).toBe('rid-2')
    expect(storeMod.store.runScript).toBe('日常登录（定时）')
  })

  it('active:false 走空闲：不登记、不动全局态', async () => {
    storeMod.store.deviceId = 'dev-3'
    storeMod.applyRunRecord(runs.normalizeActiveRunResponse({ active: false }))
    expect(storeMod.getActiveRun('dev-3')).toBeNull()
    expect(storeMod.store.running).toBe(false)
    expect(storeMod.store.runId).toBeNull()
  })
})

// ---------- ③ 202 快速返回流程 ----------
describe('③ 启动 202 快速返回（run_id 即刻主键，不等终态）', () => {
  it('normalizeStartReply：新契约取 {run_id,state}，旧后端 {ok:true}/空体 → null（走降级句柄）', () => {
    expect(runs.normalizeStartReply({ ok: true })).toBeNull()
    expect(runs.normalizeStartReply(undefined)).toBeNull()
    expect(runs.normalizeStartReply({ run_id: 'r7', state: 'starting' })).toEqual({ run_id: 'r7', state: 'starting' })
    expect(runs.normalizeStartReply({ run_id: 'r7', state: 'bizarre' }).state).toBe('starting')
  })

  it('POST run 202 立即 resolve：一次请求即点亮运行态（后续仅靠查询推进状态机）', async () => {
    fetch.mockResolvedValueOnce(res(202, { run_id: 'run-fast', state: 'starting' }))
    storeMod.store.deviceId = 'dev-4'
    const rep = await api.runScript('p/fast.yml', 'dev-4')
    expect(fetch).toHaveBeenCalledTimes(1)
    const [url, opt] = fetch.mock.calls[0]
    expect(url).toBe('/api/scripts/p%2Ffast.yml/run')
    expect(opt.method).toBe('POST')
    const st = runs.normalizeStartReply(rep)
    expect(st).toEqual({ run_id: 'run-fast', state: 'starting' })
    storeMod.applyRunRecord({ run_id: st.run_id, state: st.state, device_id: 'dev-4', script_id: 'p/fast.yml', display: '快速脚本' })
    expect(storeMod.store.running).toBe(true)
    expect(storeMod.getActiveRun('dev-4')?.script_id).toBe('p/fast.yml')
  })

  it('任务立即执行 202 {run_id} → shortRunId 截取组成「已触发（run xxxxxxxx）」提示', async () => {
    fetch.mockResolvedValueOnce(res(202, { run_id: '550e8400-e29b-41d4-a716-446655440000' }))
    const rep = await api.runTaskNow('task-1')
    expect(fetch.mock.calls[0][0]).toBe('/api/tasks/task-1/run')
    expect(rep.run_id).toBeTruthy()
    const tip = `已触发（run ${runs.shortRunId(rep.run_id)}）`
    expect(tip).toBe('已触发（run 550e8400）')
    expect(runs.shortRunId('')).toBe('')
  })
})

// ---------- ④ cancel 后状态机迁移 ----------
describe('④ 取消与终态状态机迁移', () => {
  it('beginCancel：running → stopping 仍属活动（反查保留、全局态不闪断），步骤提示转「正在停止…」', async () => {
    storeMod.store.deviceId = 'dev-5'
    storeMod.applyRunRecord({ run_id: 'rr', state: 'running', device_id: 'dev-5', script_id: 'p/c.yml' })
    const m = storeMod.beginCancel('rr')
    expect(m.state).toBe('stopping')
    expect(storeMod.getActiveRun('dev-5')?.state).toBe('stopping')
    expect(storeMod.store.running).toBe(true)
    expect(storeMod.store.runStep).toBe('正在停止…')
  })

  it('查询回 cancelled 终态 → 归档 last、清反查标记、全局态复位（runScriptId 一并清理）', async () => {
    storeMod.store.deviceId = 'dev-5'
    storeMod.applyRunRecord({ run_id: 'rr', state: 'stopping', device_id: 'dev-5', script_id: 'p/c.yml' })
    storeMod.store.runScriptId = 'p/c.yml'
    const done = storeMod.applyRunRecord({ run_id: 'rr', state: 'cancelled', device_id: 'dev-5', script_id: 'p/c.yml' })
    expect(done.state).toBe('cancelled')
    expect(storeMod.runRegistry.last?.run_id).toBe('rr')
    expect(storeMod.getActiveRun('dev-5')).toBeNull()
    expect(storeMod.store.running).toBe(false)
    expect(storeMod.store.runId).toBeNull()
    expect(storeMod.store.runScriptId).toBeNull()
  })

  it('迟到的非终态刷新打在已终态记录上：保留终态结果，不复活反查/全局态', async () => {
    storeMod.store.deviceId = 'dev-6'
    storeMod.applyRunRecord({ run_id: 'old', state: 'failed', device_id: 'dev-6', script_id: 'p/x.yml', error: 'boom' })
    expect(storeMod.store.running).toBe(false)
    const again = storeMod.applyRunRecord({ run_id: 'old', state: 'running', device_id: 'dev-6', script_id: 'p/x.yml' })
    expect(again.state).toBe('failed')
    expect(again.error).toBe('boom')
    expect(storeMod.getActiveRun('dev-6')).toBeNull()
    expect(storeMod.runRegistry.last?.state).toBe('failed')
    expect(storeMod.store.running).toBe(false)
  })

  it('cancelRun 契约寻址 POST /api/runs/:id/cancel；端点缺失（404/网络错）可判定降级', async () => {
    fetch.mockResolvedValueOnce(res(202, { cancelling: true }))
    await api.cancelRun('xyz')
    const [url, opt] = fetch.mock.calls[0]
    expect(url).toBe('/api/runs/xyz/cancel')
    expect(opt.method).toBe('POST')
    fetch.mockResolvedValueOnce(res(404, { error: 'not found' }))
    let e404 = null
    await api.cancelRun('xyz').catch(e => { e404 = e })
    expect(e404.status).toBe(404)
    expect(runs.isMissingEndpointError(e404)).toBe(true)
    expect(runs.isMissingEndpointError(new TypeError('Failed to fetch'))).toBe(true)
    // 5xx 是真实服务端错误：api 层抛出的错误带 status，不得静默降级
    const e500 = Object.assign(new Error('internal'), { status: 500 })
    expect(runs.isMissingEndpointError(e500)).toBe(false)
  })
})

// ---------- ⑤ started_at 本地化展示 ----------
describe('⑤ started_at ISO → 本地时区展示', () => {
  it('UTC 时间戳格式化为本地 YYYY-MM-DD HH:mm:ss（与原生本地 getter 对齐，跨时区稳定）', () => {
    const iso = '2026-08-27T07:30:05Z'
    const d = new Date(iso)
    const p = n => String(n).padStart(2, '0')
    const expected = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
    expect(runs.formatLocalTime(iso)).toBe(expected)
    expect(expected).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/)
  })

  it('非法/空输入兜底：null 为空串、非时间字符串原样返回', () => {
    expect(runs.formatLocalTime('')).toBe('')
    expect(runs.formatLocalTime(null)).toBe('')
    expect(runs.formatLocalTime('昨天早上')).toBe('昨天早上')
    expect(runs.formatLocalTime('2026-13-99 99:99')).toBe('2026-13-99 99:99')
  })

  it('冲突文案中的开始时间即本地化结果（与 ①联动的回归锚点）', () => {
    const started = '2026-08-26T19:00:00Z'
    expect(runs.describeConflict({ script_id: 'a.yml', source: 'manual', started_at: started }))
      .toContain(`开始于 ${runs.formatLocalTime(started)}`)
  })
})
