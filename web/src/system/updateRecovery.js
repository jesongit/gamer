import { reactive } from 'vue'
import { isAuthed, handleUnauthorized } from '../auth'

export const recovery = reactive({ message: '', ready: false, waiting: false })
export const buildIdentity = info => [info?.app?.version, info?.app?.commit, info?.app?.built_at].join('|')
export function canRefreshUpdate(before, info, update) {
  return !!before && buildIdentity(info) !== before && update?.state === 'idle' && !update.last_error
}

export function refreshUpdatedPage() {
  const event = new Event('gamer-before-update-reload', { cancelable: true })
  window.dispatchEvent(event)
  if (event.defaultPrevented) {
    recovery.message = '新版已就绪。当前有未保存内容，请先保存，再刷新页面。'
    recovery.ready = true
    return false
  }
  window.dispatchEvent(new Event('gamer-update-reload'))
  window.location.reload()
  return true
}

/** Lives above the router so leaving Settings never interrupts recovery. */
export function startUpdateRecovery() {
  let stopped = false, timer, before = '', boot = '', disconnected = false
  let edited = false
  const markEdited = event => {
    if (isAuthed() && event.target?.matches?.('input:not([type=password]), textarea, [contenteditable=true]')) edited = true
  }
  const clearEdited = () => { edited = false }
  document.addEventListener('input', markEdited)
  window.addEventListener('hashchange', clearEdited)
  async function tick() {
    if (stopped) return
    if (isAuthed()) {
      try {
        const responses = await Promise.all(['/api/system/info', '/api/system/update'].map(url => fetch(url, { cache: 'no-store', signal: AbortSignal.timeout(5000) })))
        if (responses.some(r => r.status === 401)) {
          recovery.message = '登录已过期，请重新登录。'
          recovery.waiting = false
          handleUnauthorized()
        } else if (responses.every(r => r.ok)) {
          const [info, update] = await Promise.all(responses.map(r => r.json()))
          if (!before) { before = buildIdentity(info); boot = info.startup?.boot_id }
          const active = ['installing', 'restarting', 'rolling_back'].includes(update.state)
          recovery.waiting = active
          if (active) recovery.message = '正在更新并重启，服务恢复后将自动刷新页面…'
          else if (update.state === 'failed' || update.state === 'manual_recovery') recovery.message = update.last_error?.message || '更新失败，请查看设置页。'
          else if (canRefreshUpdate(before, info, update)) {
            recovery.ready = true
            recovery.message = '新版已就绪，请保存编辑内容后刷新。'
            if (!edited) { stopped = refreshUpdatedPage() }
          } else if (disconnected || boot !== info.startup?.boot_id) {
            recovery.message = ''
            window.dispatchEvent(new Event('gamer-service-restored'))
          }
          boot = info.startup?.boot_id
          disconnected = false
        } else throw new Error('service unavailable')
      } catch {
        disconnected = true
        recovery.waiting = true
        recovery.message = '服务暂时不可用，正在等待自动重连…'
      }
    }
    if (!stopped) timer = setTimeout(tick, recovery.waiting ? 2000 : 10000)
  }
  tick()
  return () => { stopped = true; clearTimeout(timer); document.removeEventListener('input', markEdited); window.removeEventListener('hashchange', clearEdited) }
}
