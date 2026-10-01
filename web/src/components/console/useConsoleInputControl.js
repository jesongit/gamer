import { computed, onScopeDispose, ref, watch } from 'vue'
import { api } from '../../api'

const NON_INPUT_TYPES = new Set(['audio', 'reset_video', 'ping', 'frame_ack'])
const PHASE_MESSAGES = {
  running: 'AI 正在控制；请先暂停 AI 并等待暂停完成，再人工操作。',
  pausing: 'AI 正在暂停，动作收尾完成前不能人工操作。',
  paused: 'AI 已暂停，可以人工操作。',
  resuming: 'AI 正在恢复，人工操作已锁定。',
  stopping: 'AI 正在停止，操作收尾完成前不能人工操作。',
}
function valueOf(source) { return typeof source === 'function' ? source() : source?.value ?? source }

/** UI mirrors the server's target input admission. It never grants ownership.
 * Switching targets invalidates late replies immediately. Unknown/failed state
 * keeps input disabled until the next successful authoritative response.
 */
export function useConsoleInputControl({ deviceId, toast, load = (id, options) => api.getInputControl(id, options),
  pollMs = 1000, timeoutMs = 5000 } = {}) {
  const targetId = computed(() => String(valueOf(deviceId) || '').trim())
  const status = ref(null), ready = ref(false), error = ref('')
  let timer, requestAbort, epoch = 0, disposed = false, lastWarning = ''
  const manualAllowed = computed(() => !!targetId.value && ready.value && status.value?.manual_allowed === true)
  const message = computed(() => {
    if (!targetId.value) return ''
    if (!ready.value) return error.value ? '无法同步控制状态，人工操作暂不可用。' : '正在读取控制状态，人工操作暂不可用。'
    return PHASE_MESSAGES[status.value?.phase] || (manualAllowed.value ? '' : '当前控制权被占用，请先暂停自动化并等待完成。')
  })
  function requireManual() {
    if (manualAllowed.value) return true
    const warning = `${targetId.value}:${status.value?.phase || 'unknown'}:${status.value?.generation ?? ''}:${ready.value}`
    if (warning !== lastWarning) { lastWarning = warning; toast?.(message.value || '请先选择设备', 'warn') }
    return false
  }
  function guardDeviceInput(obj) {
    const type = typeof obj === 'string' ? obj : obj?.type
    return NON_INPUT_TYPES.has(String(type || '').toLowerCase()) || requireManual()
  }
  async function poll(id, token) {
    if (disposed || !id || token !== epoch) return
    const controller = new AbortController()
    requestAbort = controller
    const timeout = setTimeout(() => controller.abort(), timeoutMs)
    try {
      const value = await load(id, { signal: controller.signal })
      if (disposed || token !== epoch || id !== targetId.value) return
      if (!value || typeof value.manual_allowed !== 'boolean' || typeof value.phase !== 'string') throw new Error('控制状态响应无效')
      status.value = value; ready.value = true; error.value = ''
      if (value.manual_allowed) lastWarning = ''
    } catch (e) {
      if (!disposed && token === epoch && id === targetId.value) { ready.value = false; error.value = e.message || '无法读取控制状态' }
    } finally {
      clearTimeout(timeout)
      if (requestAbort === controller) requestAbort = null
      if (!disposed && token === epoch && id === targetId.value) timer = setTimeout(() => poll(id, token), pollMs)
    }
  }
  function refresh() {
    clearTimeout(timer); requestAbort?.abort(); epoch++
    return poll(targetId.value, epoch)
  }
  const stopWatch = watch(targetId, id => {
    clearTimeout(timer); requestAbort?.abort(); epoch++
    status.value = null; ready.value = false; error.value = ''; lastWarning = ''
    void poll(id, epoch)
  }, { immediate: true, flush: 'sync' })
  function stop() { disposed = true; epoch++; clearTimeout(timer); requestAbort?.abort(); stopWatch() }
  onScopeDispose(stop)
  return { status, ready, error, manualAllowed, message, requireManual, guardDeviceInput, refresh, stop }
}
