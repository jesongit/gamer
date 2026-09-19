import { getCurrentScope, onScopeDispose, readonly, shallowRef } from 'vue'

const current = shallowRef(null)
export const confirmation = readonly(current)

export function finishConfirmation(accepted = false) {
  const request = current.value
  if (!request) return
  current.value = null
  request.resolve(accepted === true)
}

/** One host, one pending decision. Disposed callers always resolve as cancelled. */
export function useConfirmDialog() {
  const owner = Symbol('confirmation-owner')
  let disposed = false
  const cancel = () => { if (current.value?.owner === owner) finishConfirmation(false) }
  if (getCurrentScope()) onScopeDispose(() => { disposed = true; cancel() })
  const confirmDialog = (message, options = {}) => {
    if (disposed || current.value) return Promise.resolve(false)
    return new Promise(resolve => {
      current.value = { ...options, title: options.title || '确认操作', message, owner, resolve }
    })
  }
  confirmDialog.cancel = cancel
  return confirmDialog
}
