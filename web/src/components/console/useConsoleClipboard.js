import { onScopeDispose, reactive, ref, watch } from 'vue'
import { shouldIgnoreKeyboardTarget } from '../../keyboard-control'

export function useConsoleClipboard({ deviceId, context, canUse, copyFromDevice, pasteText, releaseKeys, focusStage, toast }) {
  const dialog = reactive({ mode: '', text: '', target: '', context: '', error: '' })
  const capture = ref(null)
  let pasteContext = '', copying = false, disposed = false
  const shortcuts = new Set()
  const current = () => context()
  const valid = (target, token) => !disposed && target === deviceId() && token === current() && canUse()
  function close() { dialog.mode = ''; dialog.text = ''; dialog.error = '' }
  function openPaste(target = deviceId()) {
    if (target !== deviceId() || !canUse()) return
    Object.assign(dialog, { mode: 'paste', text: '', target, context: current(), error: '' })
  }
  async function writeClipboard(text) {
    if (navigator.clipboard?.writeText) {
      try { await navigator.clipboard.writeText(text); return true } catch { /* HTTP/permission fallback below */ }
    }
    const previous = document.activeElement
    const field = document.createElement('textarea')
    field.value = text
    field.style.cssText = 'position:fixed;left:0;top:0;width:1px;height:1px;opacity:0'
    document.body.append(field)
    try { field.select(); return document.execCommand?.('copy') === true }
    finally { field.remove(); previous?.focus?.({ preventScroll: true }) }
  }
  async function copy() {
    if (copying || !canUse()) return
    copying = true
    const target = deviceId(), token = current()
    try {
      const result = await copyFromDevice(target)
      if (!valid(target, token)) return
      const text = result?.text
      if (!text) { toast('请先在游戏中选中可复制的文字', 'warn'); return }
      if (await writeClipboard(text)) { if (valid(target, token)) toast('已复制到电脑剪贴板', 'success') }
      else if (valid(target, token)) Object.assign(dialog, { mode: 'copy', text, target, context: token, error: '' })
    } catch (error) { if (valid(target, token)) toast(`复制失败：${error.message}`, 'error') }
    finally { copying = false }
  }
  function handleKeyDown(event) {
    if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey || !['KeyC', 'KeyV'].includes(event.code)) return false
    if (!canUse()) return false
    shortcuts.add(event.code)
    if (event.code === 'KeyC') event.preventDefault()
    if (event.repeat) return true
    releaseKeys()
    if (event.code === 'KeyC') void copy()
    else {
      pasteContext = current()
      // Native paste events work on HTTP too; use an editable target for all browsers.
      capture.value?.focus({ preventScroll: true })
    }
    return true
  }
  function handleKeyUp(event) {
    const handled = shortcuts.delete(event.code)
    if (handled && event.code === 'KeyV' && document.activeElement === capture.value) focusStage()
    return handled
  }
  function handlePaste(event) {
    const captured = event.target === capture.value
    if (captured) { event.preventDefault(); capture.value.value = '' }
    if (!captured && shouldIgnoreKeyboardTarget(event.target)) return
    if (!canUse() || (captured && pasteContext !== current())) return
    event.preventDefault()
    const text = event.clipboardData?.getData('text/plain') || ''
    pasteText(text, deviceId())
    if (captured) focusStage()
  }
  async function submit(text) {
    if (!valid(dialog.target, dialog.context)) { close(); return }
    if (dialog.mode === 'paste') { if (pasteText(text, dialog.target)) { close(); focusStage() } }
    else if (await writeClipboard(dialog.text)) { close(); focusStage(); toast('已复制到电脑剪贴板', 'success') }
    else dialog.error = '请选中文本，按 Ctrl+C 复制。'
  }
  watch(current, close, { flush: 'sync' })
  onScopeDispose(() => { disposed = true; close() })
  return { dialog, capture, openPaste, close, submit, handleKeyDown, handleKeyUp, handlePaste }
}
