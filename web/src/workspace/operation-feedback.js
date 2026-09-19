import { reactive } from 'vue'

// Only the latest user operation per owner is retained. This is not a runtime monitor.
export function createOperationFeedback() {
  const state = reactive({ core: null, plugins: {} })
  let serial = 0
  const tickets = new Map()
  const invalidate = owner => { tickets.set(owner, (tickets.get(owner) || 0) + 1); return tickets.get(owner) }
  function normalize(message) {
    if (!message || !String(message.text || '').trim()) return null
    return { id: ++serial, text: String(message.text).slice(0, 1000), tone: ['error', 'success', 'warn'].includes(message.tone) ? message.tone : '',
      actions: (Array.isArray(message.actions) ? message.actions : []).slice(0, 4).map(a => ({ label: String(a.label || '').slice(0, 80), copy: a.copy == null ? undefined : String(a.copy).slice(0, 10000), detail: a.detail == null ? undefined : String(a.detail).slice(0, 10000), run: typeof a.run === 'function' ? a.run : undefined })) }
  }
  return { state,
    setCore(message) { invalidate(''); state.core = normalize(message) },
    setPlugin(owner, message) { if (owner) { invalidate(owner); state.plugins[owner] = normalize(message) } },
    clearPlugin(owner) { invalidate(owner); delete state.plugins[owner] },
    begin(owner = '') {
      const ticket = invalidate(owner)
      if (owner && !Object.hasOwn(state.plugins, owner)) state.plugins[owner] = null
      return message => {
        if (tickets.get(owner) !== ticket) return false
        if (owner) state.plugins[owner] = normalize(message)
        else state.core = normalize(message)
        return true
      }
    },
  }
}
export const OPERATION_FEEDBACK_KEY = Symbol('gamer-operation-feedback')

export function operationReporter(feedback, owner, fallback) {
  return () => {
    const finish = feedback?.begin(owner)
    return (text, tone) => {
      if (!finish) return fallback?.(text, tone)
      return finish({ text, tone, actions: tone === 'error' ? [{ label: '详情', detail: text }, { label: '复制', copy: text }] : [] })
    }
  }
}
