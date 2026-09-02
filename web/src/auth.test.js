// 鉴权会话层单测（stub fetch / localStorage / location，node 环境不引新依赖）
// 覆盖三类主路径：未认证跳转（守卫 + 401 拦截）/ 认证放行（探测/登录成功恢复）/ 退出清理（幂等成败均清态）
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

function jsonRes(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: h => (/^content-type$/i.test(h) ? 'application/json' : null) },
    json: async () => body,
  }
}

let auth

beforeEach(async () => {
  vi.resetModules()
  vi.stubGlobal('fetch', vi.fn(async () => jsonRes(401, {})))
  vi.stubGlobal('localStorage', lsStub())
  vi.stubGlobal('location', { hash: '' })
  auth = await import('../src/auth')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('formatRetryCountdown（429 倒计时文案）', () => {
  it.each([
    [0, '0 秒'],
    [45, '45 秒'],
    [60, '1 分钟'],
    [95, '1 分 35 秒'],
    ['7', '7 秒'],
    [-3, '0 秒'],
    [undefined, '0 秒'],
    [NaN, '0 秒'],
  ])('%p → %p', (input, want) => {
    expect(auth.formatRetryCountdown(input)).toBe(want)
  })

  it('小数向下取整', () => {
    expect(auth.formatRetryCountdown(59.9)).toBe('59 秒')
    expect(auth.formatRetryCountdown(120.5)).toBe('2 分钟')
  })
})

describe('sanitizeRedirect（回跳目标校验）', () => {
  it('应用内路径原样保留（含查询串）', () => {
    expect(auth.sanitizeRedirect('/tasks?tab=run')).toBe('/tasks?tab=run')
  })
  it('协议相对地址 //evil.com 挡为默认回跳', () => {
    expect(auth.sanitizeRedirect('//evil.com')).toBe('/console')
  })
  it('绝对 URL http(s):// 挡为默认回跳', () => {
    expect(auth.sanitizeRedirect('http://evil.com/x')).toBe('/console')
    expect(auth.sanitizeRedirect('https://evil.com')).toBe('/console')
  })
  it('非字符串/空值挡为回跳', () => {
    expect(auth.sanitizeRedirect(undefined)).toBe('/console')
    expect(auth.sanitizeRedirect(null)).toBe('/console')
    expect(auth.sanitizeRedirect('')).toBe('/console')
  })
  it('可指定自定义回跳兜底', () => {
    expect(auth.sanitizeRedirect(undefined, '/settings')).toBe('/settings')
  })
})

describe('resolveGuardTarget（路由守卫决策纯函数）', () => {
  it('未认证访问受保护页 → 重定向 login 并带 redirect 回跳参数', () => {
    expect(auth.resolveGuardTarget(false, 'Tasks', '/tasks?x=1'))
      .toEqual({ path: '/login', query: { redirect: '/tasks?x=1' } })
  })
  it('未认证进入根路径 → login 不带多余 redirect', () => {
    const t = auth.resolveGuardTarget(false, undefined, '/')
    expect(t.path).toBe('/login')
    expect(t.query).toBeFalsy()
  })
  it('已认证访问受保护页 → 放行', () => {
    expect(auth.resolveGuardTarget(true, 'Console', '/console')).toBe(true)
  })
  it('游客已在登录页 → 放行（避免循环）', () => {
    expect(auth.resolveGuardTarget(false, 'Login', '/login')).toBe(true)
  })
  it('已认证访问登录页 → 送回控制台', () => {
    expect(auth.resolveGuardTarget(true, 'Login', '/login')).toEqual({ path: '/console' })
  })
})

describe('probeSession（GET /api/session 探测）', () => {
  it('200 authenticated:true → 放行并记录 username', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { authenticated: true, username: 'admin' }))
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(auth.session.username).toBe('admin')
    expect(fetch.mock.calls[0][0]).toBe('/api/session')
  })

  it('结论缓存：二次导航不再重复探测', async () => {
    vi.mocked(fetch).mockResolvedValue(jsonRes(200, { authenticated: true, username: 'admin' }))
    await auth.probeSession()
    await auth.probeSession()
    await auth.probeSession()
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('401 → 未认证，内存态清空', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(401, { error: 'unauthorized' }))
    await expect(auth.probeSession()).resolves.toBe(false)
    expect(auth.session.username).toBeNull()
  })

  it('5xx → 结论未知：放行导航且不缓存（api 层 401 拦截兜底）', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(500, {}))
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(auth.session.username).toBeNull()
  })

  it('网络错误/超时 → 结论未知：放行且不缓存，下次导航重新探测（不永久缓存未认证卡死导航）', async () => {
    vi.mocked(fetch).mockRejectedValueOnce(new Error('ECONNREFUSED'))
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(auth.session.username).toBeNull()
    // 结论未缓存 → 下次导航重试；服务恢复即恢复认证态
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { authenticated: true, username: 'admin' }))
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(auth.session.username).toBe('admin')
  })
})

describe('login（POST /api/login）', () => {
  it('请求体符合契约 {username,password}；成功回包记录用户名', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { ok: true, username: 'root' }))
    const r = await auth.login('root', 'pw123')
    expect(r).toEqual({ ok: true, username: 'root' })
    expect(JSON.parse(fetch.mock.calls[0][1].body)).toEqual({ username: 'root', password: 'pw123' })
    expect(fetch.mock.calls[0][1].method).toBe('POST')
    expect(fetch.mock.calls[0][0]).toBe('/api/login')
    expect(auth.session.username).toBe('root')
  })

  it('401 invalid_credentials → 结构化失败码', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(401, { error: 'invalid_credentials' }))
    const r = await auth.login('a', 'b')
    expect(r).toEqual({ ok: false, code: 'invalid_credentials' })
    expect(auth.session.username).toBeNull()
  })

  it('429 too_many_attempts → 带 retry_after 秒数', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(429, { error: 'too_many_attempts', retry_after: 30 }))
    const r = await auth.login('a', 'b')
    expect(r.ok).toBe(false)
    expect(r.code).toBe('too_many_attempts')
    expect(r.retryAfter).toBe(30)
  })

  it('429 缺 retry_after 字段 → 至少倒计时 1 秒', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(429, {}))
    const r = await auth.login('a', 'b')
    expect(r.retryAfter).toBeGreaterThanOrEqual(1)
  })

  it('网络异常 → network_error 不抛出', async () => {
    vi.mocked(fetch).mockRejectedValueOnce(new TypeError('Failed to fetch'))
    const r = await auth.login('a', 'b')
    expect(r).toEqual({ ok: false, code: 'network_error' })
  })

  it('403 forbidden_origin → 结构化失败码（代理改写 Host 触发同源校验拒绝）', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(403, { error: 'forbidden_origin' }))
    const r = await auth.login('a', 'b')
    expect(r).toEqual({ ok: false, code: 'forbidden_origin' })
    expect(auth.session.username).toBeNull()
  })

  it('非契约状态（如 500）→ 归类 http_NNN', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(500, { error: 'boom' }))
    const r = await auth.login('a', 'b')
    expect(r).toEqual({ ok: false, code: 'http_500' })
  })
})

describe('首次设置密码（GET/POST /api/auth/setup）', () => {
  it('状态接口返回 setup_required', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { setup_required: true }))
    await expect(auth.getSetupStatus()).resolves.toEqual({ ok: true, setupRequired: true })
    expect(fetch.mock.calls[0][0]).toBe('/api/auth/setup')
  })

  it('设置成功提交确认密码并记录自动登录会话', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { ok: true, username: 'admin' }))
    await expect(auth.setupInitialPassword('strong-pass', 'strong-pass'))
      .resolves.toEqual({ ok: true, username: 'admin' })
    const [, options] = fetch.mock.calls[0]
    expect(options.method).toBe('POST')
    expect(JSON.parse(options.body)).toEqual({ password: 'strong-pass', confirm_password: 'strong-pass' })
    expect(auth.session.username).toBe('admin')
  })

  it('设置接口错误透传结构化错误码', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(400, { error: 'password_mismatch' }))
    await expect(auth.setupInitialPassword('a', 'b'))
      .resolves.toEqual({ ok: false, code: 'password_mismatch' })
  })
})

describe('doLogout（退出清理）', () => {
  it('204 成功 → POST /api/logout、清本地态、落 #/login', async () => {
    localStorage.setItem('gb_device_id', 'dev-1')
    vi.stubGlobal('location', { hash: '#/tasks' })
    vi.mocked(fetch).mockResolvedValue({ ...jsonRes(204, null), headers: { get: () => null } })
    await auth.doLogout()
    expect(fetch).toHaveBeenCalledWith('/api/logout', { method: 'POST' })
    expect(localStorage.getItem('gb_device_id')).toBeNull()
    expect(auth.session.username).toBeNull()
    expect(location.hash).toBe('#/login')
  })

  it('请求失败/断网也必须清理（幂等语义），并回到登录页', async () => {
    vi.stubGlobal('location', { hash: '#/console' })
    vi.mocked(fetch).mockRejectedValue(new Error('network down'))
    await auth.doLogout()
    expect(location.hash).toBe('#/login')
    await expect(auth.probeSession()).resolves.toBe(false)
  })

  it('已在登录页时不改写 hash（防重复跳转）', async () => {
    vi.stubGlobal('location', { hash: '#/login?redirect=%2Ftasks' })
    vi.mocked(fetch).mockResolvedValue(jsonRes(204, null))
    await auth.doLogout()
    expect(location.hash).toBe('#/login?redirect=%2Ftasks')
  })
})

describe('handleUnauthorized（全站 401 拦截）', () => {
  it('清本地界面缓存并带回跳参数跳 #/login；认证不读取本地 token', () => {
    localStorage.setItem('gb_device_id', 'dev-9')
    vi.stubGlobal('location', { hash: '#/devices?tab=a' })
    auth.handleUnauthorized()
    expect(localStorage.getItem('gb_device_id')).toBeNull()
    expect(location.hash).toBe(`#/login?redirect=${encodeURIComponent('/devices?tab=a')}`)
  })

  it('判死后探测短路：probeSession 直接 false 且不发请求', async () => {
    vi.stubGlobal('location', { hash: '#/tasks' })
    auth.handleUnauthorized()
    vi.mocked(fetch).mockClear()
    await expect(auth.probeSession()).resolves.toBe(false)
    expect(fetch).not.toHaveBeenCalled()
  })

  it('已在登录页只清理状态、不改 hash（防循环跳转）', () => {
    vi.stubGlobal('location', { hash: '#/login?redirect=%2Flogs' })
    localStorage.setItem('gb_device_id', 'dev-9')
    auth.handleUnauthorized()
    expect(location.hash).toBe('#/login?redirect=%2Flogs')
    expect(localStorage.getItem('gb_device_id')).toBeNull()
  })
})

describe('认证放行与状态恢复', () => {
  it('被 401 判死后重新登录成功 → 探测恢复 true，守卫决策转为放行', async () => {
    vi.stubGlobal('location', { hash: '#/tasks' })
    auth.handleUnauthorized()
    await expect(auth.probeSession()).resolves.toBe(false)

    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { ok: true, username: 'admin' }))
    const r = await auth.login('admin', 'admin123')
    expect(r.ok).toBe(true)
    // 守卫读到的缓存结论已翻转为已认证
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(auth.resolveGuardTarget(await auth.probeSession(), 'Tasks', '/tasks')).toBe(true)
  })

  it('守卫链路端到端：会话有效 → 深链接直接放行（无 redirect 需求）', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { authenticated: true, username: 'op' }))
    const authed = await auth.probeSession()
    expect(auth.resolveGuardTarget(authed, 'Logs', '/logs?level=warn')).toBe(true)
  })
})

describe('Cookie 会话唯一认证来源', () => {
  it('登录只提交用户名密码，不写 token；认证态来自服务端成功响应', async () => {
    localStorage.setItem('gb_sidebar_collapsed', '1')
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { ok: true, username: 'cookie-user' }))
    await expect(auth.login('cookie-user', 'pw')).resolves.toEqual({ ok: true, username: 'cookie-user' })
    const [, options] = fetch.mock.calls[0]
    expect(JSON.parse(options.body)).toEqual({ username: 'cookie-user', password: 'pw' })
    expect(options.headers.Authorization).toBeUndefined()
    expect(localStorage.getItem('gb_sidebar_collapsed')).toBe('1')
    expect(auth.session.username).toBe('cookie-user')
    expect(auth).not.toHaveProperty('purgeLegacySessionKeys')
  })

  it('session 探测只消费服务端 authenticated/username，Cookie 由浏览器管理', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonRes(200, { authenticated: true, username: 'admin' }))
    await expect(auth.probeSession()).resolves.toBe(true)
    expect(fetch.mock.calls[0][0]).toBe('/api/session')
    expect(fetch.mock.calls[0][1].credentials).toBeUndefined()
    expect(fetch.mock.calls[0][1].headers).toBeUndefined()
  })
})
