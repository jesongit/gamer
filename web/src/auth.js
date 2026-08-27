// 鉴权会话层（阶段 2 SEC 契约，钉死不复述第二套口径）：
// - 登录   POST /api/login  {username,password} → 200 Set-Cookie gb_session / 401 invalid_credentials / 429 too_many_attempts{retry_after}
// - 探测   GET  /api/session → 200 {authenticated:true,username} / 401
// - 退出   POST /api/logout  → 204 幂等（成败均清本地态）
// 同源部署 SameSite=Strict：fetch 不设 credentials（默认同源自动携带/写回 Cookie），不引 CSRF token。
// 会话态只存内存（session.username），刷新后由路由守卫重新探测；不再向 localStorage 写伪 token，
// 旧键 gb_token 见到即删（purgeLegacySessionKeys）。
// 供 api.js 使用的全站 401 拦截：handleUnauthorized() 清缓存态并跳 #/login（保留回跳参数）。
import { reactive } from 'vue'

export const session = reactive({
  username: null           // null = 未认证；登录成功/探测通过后为用户名
})

export function isAuthed() {
  return session.username !== null
}

// ---- 内部探测/拦截状态（模块级单例，登录/登出/401 时翻转，避免陈旧探测结果打转）----
let _probe = null                       // 最近一次 GET /api/session 的结论（null=尚未探测）
let _forcedUnauthed = false             // 401 拦截器判死后短路后续探测

function markAuthed(name) {
  session.username = name || ''
  _forcedUnauthed = false
  _probe = Promise.resolve(true)
}

function markUnauthed(forced) {
  session.username = null
  _forcedUnauthed = !!forced
  _probe = Promise.resolve(false)
}

// ---- localStorage 工具（node 单测环境可能缺此全局，统一兜底）----
function safeRemove(key) {
  try { localStorage.removeItem(key) } catch (e) { /* 无 localStorage 环境（单测/隐私模式）忽略 */ }
}

// 旧版伪 token 清退：不读取、不复用，见到即删（应用启动、登录、登出、401 时都会触发一次）
export function purgeLegacySessionKeys() {
  safeRemove('gb_token')
}

// 清理本地缓存状态：会话内存态 + 登录期的界面缓存（设备选择等），供登出与 401 拦截共用
export function clearLocalState() {
  session.username = null
  safeRemove('gb_device_id')
  purgeLegacySessionKeys()
}

function currentHashPath() {
  const raw = String(typeof location === 'undefined' ? '' : location.hash || '')
  return raw.startsWith('#') ? raw.slice(1) : raw
}

function isOnLoginPage() {
  const p = currentHashPath()
  return p === '/login' || p.startsWith('/login?') || p.startsWith('/login#')
}

// ---- 登录 / 退出 / 探测 ----

// 登录：返回结构化结果而非抛错，调用方据 code 定制文案
//   {ok:true,username} | {ok:false,code:'invalid_credentials'|'too_many_attempts'|'network_error'|http_NNN, retryAfter?}
export async function login(username, password) {
  let r
  try {
    r = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password })
    })
  } catch (e) {
    return { ok: false, code: 'network_error' }
  }
  const body = await r.json().catch(() => ({}))
  if (r.ok) {
    markAuthed(body.username || username)
    return { ok: true, username: session.username }
  }
  if (r.status === 429) {
    return { ok: false, code: 'too_many_attempts', retryAfter: Math.max(1, Math.floor(Number(body.retry_after) || 1)) }
  }
  if (r.status === 401) return { ok: false, code: 'invalid_credentials' }
  return { ok: false, code: `http_${r.status}` }
}

// 启动/首次导航探测会话（结论缓存：后续导航复用；login/doLogout/handleUnauthorized 翻转缓存，
// 会话过期则由 api 层 401 拦截刷新为未认证——不会拿着陈旧结论放行）
export function probeSession() {
  if (_probe) return _probe
  _probe = (async () => {
    if (_forcedUnauthed) return false               // 已被 401 判死，别再问
    try {
      const r = await fetch('/api/session')
      if (r.ok) {
        const b = await r.json().catch(() => ({}))
        markAuthed(b.username)
        return true
      }
    } catch (e) { /* 服务不可达按未认证处理（后端未就绪属预期态） */ }
    markUnauthed(_forcedUnauthed)                    // 保留 forced 标志不被普通 401 冲掉
    return false
  })()
  return _probe
}

// 退出登录：契约要求幂等；请求失败（网络断/重复调用）也必须清本地态回登录页
export async function doLogout() {
  try { await fetch('/api/logout', { method: 'POST' }) } catch (e) { /* 幂等：失败同样视为已退出 */ }
  markUnauthed(false)
  clearLocalState()
  if (!isOnLoginPage()) location.hash = '#/login'
}

// ---- 全站 401 拦截（api.js 在每个非豁免响应上调用）----
// 清本地缓存态 → 跳 #/login 并保留当前地址做回跳；已在登录页则只清理，防止循环跳转。
export function handleUnauthorized() {
  clearLocalState()
  markUnauthed(true)
  if (isOnLoginPage()) return
  const cur = currentHashPath() || '/'
  location.hash = `#/login?redirect=${encodeURIComponent(cur)}`
}

// ---- 纯函数工具（单测覆盖）----

// 路由守卫决策：true=放行；对象=vuerouter 重定向目标
export function resolveGuardTarget(authed, toName, toFullPath) {
  if (toName === 'Login') return authed ? { path: '/console' } : true
  if (!authed) {
    return { path: '/login', query: toFullPath && toFullPath !== '/' ? { redirect: toFullPath } : undefined }
  }
  return true
}

// 回跳目标校验：只放行应用内绝对路径；挡掉开放重定向（协议相对 //evil.com、绝对 URL、非字符串）
export function sanitizeRedirect(target, fallback = '/console') {
  if (typeof target !== 'string' || !target.startsWith('/') || target.startsWith('//')) return fallback
  return target
}

// 429 倒计时文案：<60 秒“N 秒”；≥60 秒“M 分(可选 S 秒)”
export function formatRetryCountdown(seconds) {
  const s = Math.max(0, Math.floor(Number(seconds) || 0))
  if (s < 60) return `${s} 秒`
  const m = Math.floor(s / 60), r = s % 60
  return r ? `${m} 分 ${r} 秒` : `${m} 分钟`
}
