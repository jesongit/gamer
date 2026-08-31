// WEB-001：system/update API client + 状态/动作矩阵 + 轮询 composable + 安装/回滚流程逻辑。
// 契约：release/contracts/system-api-v1.md（冻结）；响应 fixture 直接取自
// release/contracts/fixtures/system-api/*.json（fixture 索引见契约 §9，26 个全量对照）。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { readFileSync } from 'node:fs'

const FIX_DIR = new URL('../../release/contracts/fixtures/system-api/', import.meta.url)

/** 读取契约 fixture 的 response 段（{status, body}） */
function fixtureRes(name) {
  return JSON.parse(readFileSync(new URL(name, FIX_DIR), 'utf8')).response
}

/** 读取完整契约 fixture（{scenario, endpoint, request, response}） */
function fixtureFull(name) {
  return JSON.parse(readFileSync(new URL(name, FIX_DIR), 'utf8'))
}

function res(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (h) => (/^content-type$/i.test(h) ? 'application/json' : null) },
    json: async () => body,
  }
}

let apiMod, states, status, flow, auth, ApiError

beforeEach(async () => {
  vi.stubGlobal('fetch', vi.fn())
  vi.stubGlobal('location', { hash: '#/settings' })
  vi.resetModules()
  apiMod = await import('./system/api')
  states = await import('./system/states')
  status = await import('./system/useSystemStatus')
  flow = await import('./system/useUpdateFlow')
  auth = await import('./auth')
  ApiError = (await import('./api')).ApiError
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

/** 按 URL 路由 stub fetch：map[url] = {status, body}（未命中一律 404 not_found） */
function mockByPath(map) {
  global.fetch.mockImplementation(async (url) => {
    const hit = map[url]
    if (!hit) return res(404, { error: 'not_found' })
    return res(hit.status, hit.body)
  })
}

describe('GET /api/system/info 与 /api/system/update：字段与契约 fixture 一致', () => {
  it('launcher 模式全量 info：逐字段透传 fixture body', async () => {
    mockByPath({ '/api/system/info': fixtureRes('system-info.success.json') })
    await expect(apiMod.systemApi.getSystemInfo()).resolves.toEqual(fixtureRes('system-info.success.json').body)
    expect(global.fetch).toHaveBeenCalledWith('/api/system/info', expect.objectContaining({ method: 'GET' }))
  })

  it('docker 降级 info：capability 全 false / strategy external 原样透传', async () => {
    mockByPath({ '/api/system/info': fixtureRes('system-info.degraded-docker.json') })
    const info = await apiMod.systemApi.getSystemInfo()
    expect(info.capabilities).toEqual({ check: false, download: false, install: false, rollback: false })
    expect(info.deployment).toEqual({ mode: 'docker', update_strategy: 'external' })
  })

  it('update status：staged / failed(signature_invalid) / failed(artifact_invalid) / manual_recovery 全部原样透传', async () => {
    for (const name of [
      'system-update.success.json',
      'system-update.failed-signature-invalid.json',
      'system-update.failed-artifact-invalid.json',
      'system-update.manual-recovery.json',
    ]) {
      vi.resetModules()
      apiMod = await import('./system/api')
      mockByPath({ '/api/system/update': fixtureRes(name) })
      await expect(apiMod.systemApi.getUpdateStatus()).resolves.toEqual(fixtureRes(name).body)
    }
  })

  it('/health/ready：匿名就绪 200 与未就绪 503 均原样透传（§8 向后兼容）', async () => {
    mockByPath({
      '/health/ready': fixtureRes('health-ready.success.json'),
    })
    await expect(apiMod.systemApi.getHealthReady()).resolves.toEqual({
      ready: true,
      checks: expect.objectContaining({ sqlite: { ok: true } }),
    })

    global.fetch.mockImplementationOnce(async () => res(503, fixtureRes('health-ready.not-ready.json').body))
    // 503「未就绪」是探针的有效结论：resolve body（body.ready=false），不抛错
    await expect(apiMod.systemApi.getHealthReady()).resolves.toEqual({ ready: false, checks: expect.any(Object) })
    // 匿名端点不触发登录跳转
    expect(location.hash).toBe('#/settings')
  })
})

describe('动作端点 202 受理语义（§4.1）', () => {
  it('check/download/install/rollback 成功 fixture：返回 {update_id, state, accepted:true}，不等待动作完成', async () => {
    const cases = [
      ['update-check.success.json', 'checkUpdate', '/api/system/update/check', 'checking'],
      ['update-download.success.json', 'downloadUpdate', '/api/system/update/download', 'downloading'],
      ['update-install.success.json', 'installUpdate', '/api/system/update/install', 'installing'],
      ['update-rollback.success.json', 'rollbackUpdate', '/api/system/update/rollback', 'rolling_back'],
    ]
    for (const [name, method, path, state] of cases) {
      global.fetch.mockReset()
      mockByPath({ [path]: fixtureRes(name) })
      await expect(apiMod.systemApi[method]()).resolves.toEqual({
        update_id: 'upd-20260831-9f3ab2c1',
        state,
        accepted: true,
      })
      expect(global.fetch).toHaveBeenCalledWith(path, expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({ 'Content-Type': 'application/json' }),
      }))
    }
  })

  it('202 body 结构破坏（缺 update_id/state 非法）→ 502 invalid_response，不给调用方误判受理', async () => {
    global.fetch.mockImplementation(async () => res(202, { ok: true }))
    await expect(apiMod.systemApi.installUpdate()).rejects.toMatchObject({
      status: 502, code: 'invalid_response',
    })
    global.fetch.mockImplementation(async () => res(202, { update_id: 'u1', state: 'exploded' }))
    await expect(apiMod.systemApi.rollbackUpdate()).rejects.toMatchObject({ code: 'invalid_response' })
  })
})

describe('PUT /api/system/update/policy（§6 整对象替换）', () => {
  const policy = { strategy: 'auto', maintenance_window: { start: '02:00', end: '06:00' }, freeze_window_minutes: 30 }

  it('成功 200 回显保存后的策略', async () => {
    mockByPath({ '/api/system/update/policy': fixtureRes('update-policy.success.json') })
    await expect(apiMod.systemApi.setUpdatePolicy(policy)).resolves.toEqual(policy)
    const [, opts] = global.fetch.mock.calls[0]
    expect(opts.method).toBe('PUT')
    expect(JSON.parse(opts.body)).toEqual(policy)
  })
})

describe('错误归一化：HTTP 状态码 + {code,message,details} 统一为 ApiError（逐 fixture 对照）', () => {
  const cases = [
    // [fixture, 请求方法, 期望 {status, code, details}]
    ['update-check.launcher-unreachable.json', 'checkUpdate', { status: 502, code: 'launcher_unreachable' }],
    ['update-download.update-not-available.json', 'downloadUpdate', { status: 409, code: 'update_not_available' }],
    ['update-install.update-busy.json', 'installUpdate', { status: 409, code: 'update_busy' }],
    ['update-install.update-not-managed.json', 'installUpdate', { status: 409, code: 'update_not_managed' }],
    ['update-rollback.rollback-unavailable.json', 'rollbackUpdate', { status: 409, code: 'rollback_unavailable' }],
  ]
  for (const [name, method, expected] of cases) {
    it(`${name} → ${expected.status} ${expected.code}`, async () => {
      // 用对应动作端点路径路由 fixture（endpoint 在完整 fixture 上）
      const full = fixtureFull(name)
      const fix = full.response
      const path = '/' + full.endpoint.split('/').slice(1).join('/')
      mockByPath({ [path]: fix })
      await expect(apiMod.systemApi[method]()).rejects.toSatisfy((e) => {
        expect(e).toBeInstanceOf(ApiError)
        expect(e.status).toBe(expected.status)
        expect(e.code).toBe(expected.code)
        expect(e.message).toBeTruthy()
        return true
      })
    })
  }

  it('update_not_ready：details.blocking 冻结数组原样透传', async () => {
    mockByPath({ '/api/system/update/install': fixtureRes('update-install.update-not-ready.json') })
    const err = await apiMod.systemApi.installUpdate().then(() => null, (e) => e)
    expect(err.status).toBe(409)
    expect(err.code).toBe('update_not_ready')
    expect(err.details).toEqual({ blocking: ['active_run', 'cron_freeze_window'] })
  })

  it('schema_incompatible：details 携带 candidate_schema / supported_range', async () => {
    mockByPath({ '/api/system/update/install': fixtureRes('update-install.schema-incompatible.json') })
    const err = await apiMod.systemApi.installUpdate().then(() => null, (e) => e)
    expect(err.status).toBe(422)
    expect(err.code).toBe('schema_incompatible')
    expect(err.details).toEqual({ candidate_schema: 4, supported_range: [1, 3] })
  })

  it('insufficient_space：details 携带 required_bytes / available_bytes（不给路径）', async () => {
    mockByPath({ '/api/system/update/download': fixtureRes('update-download.insufficient-space.json') })
    const err = await apiMod.systemApi.downloadUpdate().then(() => null, (e) => e)
    expect(err.status).toBe(507)
    expect(err.code).toBe('insufficient_space')
    expect(err.details).toEqual({ required_bytes: 2684354560, available_bytes: 1073741824 })
  })

  it('policy 400 invalid_argument：details.field 指明非法字段', async () => {
    mockByPath({ '/api/system/update/policy': fixtureRes('update-policy.invalid-argument.json') })
    const err = await apiMod.systemApi
      .setUpdatePolicy({ strategy: 'auto', maintenance_window: { start: '02:00', end: '02:00' }, freeze_window_minutes: 30 })
      .then(() => null, (e) => e)
    expect(err.status).toBe(400)
    expect(err.code).toBe('invalid_argument')
    expect(err.details).toEqual({ field: 'maintenance_window' })
  })

  it('未登录 401（GET 与状态变更方法同样）→ code=unauthorized 并交给全站 401 拦截跳登录', async () => {
    for (const [name, path, call] of [
      ['system-info.unauthorized.json', '/api/system/info', () => apiMod.systemApi.getSystemInfo()],
      ['system-update.unauthorized.json', '/api/system/update', () => apiMod.systemApi.getUpdateStatus()],
      ['update-install.unauthorized.json', '/api/system/update/install', () => apiMod.systemApi.installUpdate()],
    ]) {
      global.fetch.mockReset()
      auth.session.username = 'admin'
      mockByPath({ [path]: fixtureRes(name) })
      await expect(call()).rejects.toMatchObject({ status: 401, code: 'unauthorized' })
      expect(String(location.hash)).toContain('#/login')
      expect(auth.session.username).toBeNull()
    }
  })

  it('跨站状态变更 403 forbidden_origin：中间件固定 body {error} 归一化', async () => {
    mockByPath({ '/api/system/update/install': fixtureRes('update-install.forbidden-origin.json') })
    await expect(apiMod.systemApi.installUpdate()).rejects.toMatchObject({
      status: 403, code: 'forbidden_origin', message: 'forbidden_origin',
    })
  })

  it('非 JSON 错误响应（404 等）→ code 回落 http_<status>；网络失败 → network_error', async () => {
    global.fetch.mockImplementation(async () => ({
      ok: false, status: 404, headers: { get: () => 'text/plain' }, json: async () => { throw new Error('not json') },
    }))
    await expect(apiMod.systemApi.getUpdateStatus()).rejects.toMatchObject({ status: 404, code: 'http_404' })

    global.fetch.mockRejectedValueOnce(new TypeError('offline'))
    await expect(apiMod.systemApi.getSystemInfo()).rejects.toMatchObject({
      name: 'ApiError', status: 0, code: 'network_error',
    })
  })
})

describe('SYSTEM_ERRORS 错误码常量表（§7 冻结：11 码 + HTTP 状态映射）', () => {
  it('恰为 11 个冻结错误码，且与契约 §7 的 HTTP 状态码一致', () => {
    expect(apiMod.SYSTEM_ERROR_CODES).toHaveLength(11)
    const frozen = {
      update_not_managed: 409,
      update_busy: 409,
      update_not_available: 409,
      update_not_ready: 409,
      signature_invalid: 422,
      artifact_invalid: 422,
      insufficient_space: 507,
      schema_incompatible: 422,
      launcher_unreachable: 502,
      rollback_unavailable: 409,
      manual_recovery_required: 409,
    }
    expect(apiMod.SYSTEM_ERROR_CODES).toEqual(Object.keys(frozen))
    for (const [code, status] of Object.entries(frozen)) {
      expect(apiMod.SYSTEM_ERRORS[code].status).toBe(status)
      expect(typeof apiMod.SYSTEM_ERRORS[code].retryable).toBe('boolean')
      expect(apiMod.SYSTEM_ERRORS[code].hint).toBeTruthy()
    }
    // update_not_managed / rollback_unavailable / manual_recovery_required：部署或事务态不变则恒定，不可重试
    expect(apiMod.SYSTEM_ERRORS.update_not_managed.retryable).toBe(false)
    expect(apiMod.SYSTEM_ERRORS.rollback_unavailable.retryable).toBe(false)
    expect(apiMod.SYSTEM_ERRORS.manual_recovery_required.retryable).toBe(false)
    expect(apiMod.INVALID_ARGUMENT).toBe('invalid_argument')
  })
})

describe('状态×动作受理矩阵（§4.2 冻结）', () => {
  it('11 态逐行与契约矩阵一致；未知状态按全拒绝保守处理', () => {
    const matrix = {
      idle:            { check: true,  download: false, install: false, rollback: false },
      checking:        { check: true,  download: false, install: false, rollback: false },
      available:       { check: true,  download: true,  install: false, rollback: false },
      downloading:     { check: true,  download: true,  install: false, rollback: false },
      staged:          { check: true,  download: true,  install: true,  rollback: true },
      waiting:         { check: true,  download: true,  install: true,  rollback: true },
      installing:      { check: false, download: false, install: false, rollback: false },
      restarting:      { check: false, download: false, install: false, rollback: false },
      failed:          { check: true,  download: true,  install: true,  rollback: true },
      rolling_back:    { check: false, download: false, install: false, rollback: false },
      manual_recovery: { check: false, download: false, install: false, rollback: false },
    }
    expect(states.UPDATE_STATES).toEqual(Object.keys(matrix))
    for (const [state, row] of Object.entries(matrix)) {
      expect(states.allowedActions(state)).toEqual(row)
    }
    expect(states.allowedActions('bogus')).toEqual({ check: false, download: false, install: false, rollback: false })
  })

  it('11 态展示元数据齐全（label/desc/tone），§5.2 detail 映射可用（含新增驻留值 checked）', () => {
    for (const s of states.UPDATE_STATES) {
      expect(states.STATE_META[s].label).toBeTruthy()
      expect(states.STATE_META[s].desc).toBeTruthy()
    }
    expect(states.DETAIL_LABELS.checked).toBe('检查完成')
    expect(states.DETAIL_LABELS.manual_recovery_required).toBe('需要人工恢复')
  })
})

describe('useSystemStatus：轮询节奏（活跃态高频 / idle 低频 / 卸载停止）', () => {
  const INFO_BODY = fixtureRes('system-info.success.json').body

  function statusBody(state, extra = {}) {
    // 内联最小 update status：字段结构与 release/contracts/fixtures/system-api/system-update.success.json 一致
    return {
      state, detail: state, update_id: state === 'idle' ? null : 'upd-20260831-9f3ab2c1',
      candidate: null, progress: null,
      policy: { strategy: 'notify', maintenance_window: { start: '02:00', end: '06:00' }, freeze_window_minutes: 30 },
      last_error: null, updated_at: '2026-08-31T12:00:00Z', ...extra,
    }
  }

  it('downloading 态高频轮询，回到 idle 转低频；fetch 失败不中断循环', async () => {
    vi.useFakeTimers()
    let state = 'downloading'
    let failNext = false
    global.fetch.mockImplementation(async (url) => {
      if (failNext && url === '/api/system/update') throw new TypeError('offline')
      return res(200, url === '/api/system/update' ? statusBody(state) : INFO_BODY)
    })
    const ctl = status.createSystemStatus({ fastMs: 1000, slowMs: 10000 })
    await ctl.refresh()
    expect(ctl.st.update.state).toBe('downloading')
    expect(ctl.st.info.app.version).toBe('0.2.0')
    expect(ctl.st.info.dependencies.ffmpeg.version).toBe('6.1.1')

    ctl.startPolling()
    await vi.advanceTimersByTimeAsync(1000) // t=0 首轮 + t=1s 高频第二轮
    const fastCalls = global.fetch.mock.calls.length
    expect(fastCalls).toBeGreaterThanOrEqual(4) // refresh(2) + ≥1 轮(2)

    // 单次 update 拉取失败：错误落位、info 不丢、循环继续
    failNext = true
    await vi.advanceTimersByTimeAsync(1000)
    expect(ctl.st.updateError).toBeInstanceOf(ApiError)
    expect(ctl.st.info.app.version).toBe('0.2.0')
    failNext = false

    state = 'idle'
    await vi.advanceTimersByTimeAsync(1000) // 本轮刷新后 state=idle → 下轮转低频
    const idleCalls = global.fetch.mock.calls.length
    await vi.advanceTimersByTimeAsync(5000) // < 10s 低频间隔：不再拉取
    expect(global.fetch.mock.calls.length).toBe(idleCalls)
    await vi.advanceTimersByTimeAsync(5000) // 到达低频间隔：恢复拉取
    expect(global.fetch.mock.calls.length).toBeGreaterThan(idleCalls)

    ctl.stopPolling()
    const stopped = global.fetch.mock.calls.length
    await vi.advanceTimersByTimeAsync(60000)
    expect(global.fetch.mock.calls.length).toBe(stopped)
    expect(ctl.st.polling).toBe(false)
  })
  // useSystemStatus 组件内卸载自动停轮询的验收在 system-components.test.js（需 happy-dom 挂载）
})

describe('useUpdateFlow：安装 202 → 断连等待 → 重连判定（WEB-004）', () => {
  const BEFORE_INFO = { app: { version: '0.2.0' }, startup: { boot_id: 'boot-1' } }

  it('安装受理 → 断连（正常路径）→ 重连后 boot_id/版本变化 + state=idle → 判定成功', async () => {
    const poll = vi.fn()
      .mockResolvedValueOnce({ ok: false }) // 服务重启中，连接被拒
      .mockResolvedValueOnce({ ok: false })
      .mockResolvedValueOnce({
        ok: true,
        info: { app: { version: '0.3.0' }, startup: { boot_id: 'boot-2' } },
        update: { state: 'idle', detail: 'committed', last_error: null },
      })
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'installing', accepted: true }) },
      pollOnce: poll,
      sleep: vi.fn().mockResolvedValue(),
    })
    const r = await f.submitInstall(BEFORE_INFO)
    expect(r.ok).toBe(true)
    expect(f.flow.phase).toBe('done')
    expect(f.flow.verdict).toBe('success')
    expect(f.flow.restarted).toBe(true)
    expect(f.flow.versionChanged).toBe(true)
    expect(poll).toHaveBeenCalledTimes(3)
  })

  it('重连后 state=failed → 判定失败并透传 last_error，不误报成功', async () => {
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'installing', accepted: true }) },
      pollOnce: vi.fn().mockResolvedValue({
        ok: true,
        info: { app: { version: '0.2.0' }, startup: { boot_id: 'boot-1' } },
        update: { state: 'failed', detail: 'failed', last_error: { code: 'signature_invalid', message: '发布清单验签失败' } },
      }),
      sleep: vi.fn().mockResolvedValue(),
    })
    await f.submitInstall(BEFORE_INFO)
    expect(f.flow.verdict).toBe('failed')
    expect(f.flow.error).toEqual({ code: 'signature_invalid', message: '发布清单验签失败', details: null })
  })

  it('waiting 期轮询返回 rolling_back/installing 一律继续等待；回滚流程 idle 即成功（不要求版本变化）', async () => {
    const api = {
      installUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'installing', accepted: true }),
      rollbackUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'rolling_back', accepted: true }),
    }
    const poll = vi.fn()
      .mockResolvedValueOnce({ ok: true, info: BEFORE_INFO, update: { state: 'rolling_back', detail: 'rolling_back', last_error: null } })
      .mockResolvedValueOnce({ ok: true, info: BEFORE_INFO, update: { state: 'idle', detail: 'idle', last_error: null } })
    const f = flow.createUpdateFlow({ api, pollOnce: poll, sleep: vi.fn().mockResolvedValue() })
    await f.submitRollback(BEFORE_INFO)
    expect(f.flow.kind).toBe('rollback')
    expect(f.flow.verdict).toBe('success')

    // 安装流：回到 idle 但无任何重启证据且 detail=idle → 不误判成功，继续等待
    const poll2 = vi.fn().mockResolvedValue({ ok: true, info: BEFORE_INFO, update: { state: 'idle', detail: 'idle', last_error: null } })
    const f2 = flow.createUpdateFlow({
      api: { installUpdate: api.installUpdate },
      pollOnce: poll2,
      sleep: vi.fn().mockResolvedValue(),
      maxTries: 3,
    })
    await f2.submitInstall(BEFORE_INFO)
    expect(f2.flow.verdict).toBe('timeout') // 有界等待耗尽：按超时提示，不误报成功也不误报失败
  })

  it('409 update_busy：只置 busy 提示等待，不轮询不重试轰炸', async () => {
    const poll = vi.fn()
    const err = new ApiError({ status: 409, code: 'update_busy', message: '已有升级事务正在进行，请等待其结束后再试' })
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockRejectedValue(err) },
      pollOnce: poll,
      sleep: vi.fn(),
    })
    const r = await f.submitInstall(BEFORE_INFO)
    expect(r.ok).toBe(false)
    expect(r.code).toBe('update_busy')
    expect(f.flow.busy).toBe(true)
    expect(f.flow.phase).toBe('idle')
    expect(poll).not.toHaveBeenCalled()
  })

  it('409 update_not_ready：归一化 error 携带 blocking 门禁列表，不进入等待', async () => {
    const err = new ApiError({
      status: 409, code: 'update_not_ready', message: '安装条件未满足',
      details: { blocking: ['active_run', 'cron_freeze_window'] },
    })
    const poll = vi.fn()
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockRejectedValue(err) },
      pollOnce: poll,
      sleep: vi.fn(),
    })
    await f.submitInstall(BEFORE_INFO)
    expect(f.flow.phase).toBe('idle')
    expect(f.flow.error.code).toBe('update_not_ready')
    expect(f.flow.error.details.blocking).toEqual(['active_run', 'cron_freeze_window'])
    expect(poll).not.toHaveBeenCalled()
  })

  it('有界等待耗尽（始终断连）→ verdict=timeout，不误报失败', async () => {
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'installing', accepted: true }) },
      pollOnce: vi.fn().mockResolvedValue({ ok: false }),
      sleep: vi.fn().mockResolvedValue(),
      maxTries: 4,
    })
    await f.submitInstall(BEFORE_INFO)
    expect(f.flow.verdict).toBe('timeout')
    expect(f.flow.tries).toBe(4)
  })

  it('等待期 cancel：停止轮询并标记 aborted（不影响后台事务语义）', async () => {
    let resolvePoll
    const poll = vi.fn(() => new Promise((r) => { resolvePoll = r }))
    const f = flow.createUpdateFlow({
      api: { installUpdate: vi.fn().mockResolvedValue({ update_id: 'u1', state: 'installing', accepted: true }) },
      pollOnce: poll,
      sleep: vi.fn(), // cancel 后循环在 poll 返回处即退出，不会走到 sleep
    })
    const p = f.submitInstall(BEFORE_INFO)
    for (let i = 0; i < 50 && f.flow.phase !== 'waiting'; i++) await new Promise((r) => setTimeout(r, 5))
    expect(f.flow.phase).toBe('waiting')
    f.cancel()
    resolvePoll({ ok: false })
    await p
    expect(f.flow.verdict).toBe('aborted')
  })

  it('judgeAfterRestart：manual_recovery 独立判定；无效/缺数据按 waiting 保守处理', () => {
    expect(flow.judgeAfterRestart({
      before: { version: '0.2.0', bootId: 'b1' },
      after: { info: BEFORE_INFO, update: { state: 'manual_recovery', detail: 'manual_recovery_required' } },
    })).toMatchObject({ verdict: 'manual_recovery' })
    expect(flow.judgeAfterRestart({ after: null })).toEqual({ verdict: 'waiting', restarted: false, versionChanged: false })
    expect(flow.judgeAfterRestart({
      before: { version: '0.2.0', bootId: 'b1' },
      after: { info: { app: { version: '0.2.0' }, startup: { boot_id: 'b1' } }, update: { state: 'staged', detail: 'staged' } },
    })).toMatchObject({ verdict: 'waiting', restarted: false, versionChanged: false })
  })
})
