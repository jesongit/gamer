// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope, reactive, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { useConsoleClipboard } from './components/console/useConsoleClipboard'
import { useConsoleDeviceManager } from './components/console/useConsoleDeviceManager'
import ConsoleClipboardDialog from './components/console/ConsoleClipboardDialog.vue'

const scopes = []
afterEach(() => { scopes.splice(0).forEach(scope => scope.stop()); vi.unstubAllGlobals(); vi.restoreAllMocks(); document.body.innerHTML = '' })
function setup() {
  const state = reactive({ id: 'phone', generation: 0, allowed: true })
  const pasteText = vi.fn(() => true), copyFromDevice = vi.fn(async () => ({ text: ' 中文🙂\n' })), toast = vi.fn()
  const scope = effectScope(); scopes.push(scope)
  const clipboard = scope.run(() => useConsoleClipboard({ deviceId: () => state.id, context: () => `${state.id}:${state.generation}`,
    canUse: () => state.allowed, copyFromDevice, pasteText, releaseKeys: vi.fn(), focusStage: vi.fn(), toast }))
  const capture = document.createElement('textarea'); document.body.append(capture); clipboard.capture.value = capture
  return { state, clipboard, pasteText, copyFromDevice, toast, capture }
}
function key(code, extra = {}) { return { code, ctrlKey: true, preventDefault: vi.fn(), ...extra } }
function paste(target, text) { return { target, preventDefault: vi.fn(), clipboardData: { getData: vi.fn(() => text) } } }

describe('游戏文本剪贴板', () => {
  it('Ctrl+V leaves native paste enabled on HTTP, focuses a capture field and sends plain text exactly once', () => {
    vi.stubGlobal('navigator', {})
    const { clipboard, pasteText, capture } = setup(), event = key('KeyV')
    expect(clipboard.handleKeyDown(event)).toBe(true)
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(document.activeElement).toBe(capture)
    const pasted = paste(capture, ' 中文🙂\n')
    clipboard.handlePaste(pasted)
    expect(pasted.preventDefault).toHaveBeenCalled()
    expect(pasteText).toHaveBeenCalledExactlyOnceWith(' 中文🙂\n', 'phone')
    expect(clipboard.handleKeyUp(key('KeyV'))).toBe(true)
  })
  it('does not hijack editor paste and drops captured text after a control generation change', () => {
    const { clipboard, state, pasteText, capture } = setup()
    const editor = document.createElement('textarea'), event = paste(editor, '编辑器文本')
    clipboard.handlePaste(event)
    expect(event.preventDefault).not.toHaveBeenCalled()
    clipboard.handleKeyDown(key('KeyV')); state.generation++
    clipboard.handlePaste(paste(capture, '旧控制权的文本'))
    expect(pasteText).not.toHaveBeenCalled()
    state.allowed = false
    expect(clipboard.handleKeyDown(key('KeyC'))).toBe(false)
  })
  it('copies the selected device text with HTTP execCommand fallback and consumes shortcut repeats', async () => {
    vi.stubGlobal('navigator', {})
    const execCommand = vi.fn(() => true); document.execCommand = execCommand
    const { clipboard, copyFromDevice, toast } = setup(), event = key('KeyC')
    expect(clipboard.handleKeyDown(event)).toBe(true)
    clipboard.handleKeyDown(key('KeyC', { repeat: true }))
    await flushPromises()
    expect(event.preventDefault).toHaveBeenCalled()
    expect(copyFromDevice).toHaveBeenCalledOnce()
    expect(execCommand).toHaveBeenCalledExactlyOnceWith('copy')
    expect(toast).toHaveBeenCalledWith('已复制到电脑剪贴板', 'success')
  })
  it('ignores copy replies after target switch and offers selectable text when clipboard writes fail', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('denied'))
    vi.stubGlobal('navigator', { clipboard: { writeText } }); document.execCommand = vi.fn(() => false)
    const { clipboard, copyFromDevice, state } = setup()
    let resolve; copyFromDevice.mockImplementationOnce(() => new Promise(done => { resolve = done }))
    clipboard.handleKeyDown(key('KeyC')); state.id = 'other'; resolve({ text: '过期内容' }); await flushPromises()
    expect(writeText).not.toHaveBeenCalled()
    clipboard.handleKeyDown(key('KeyC')); await flushPromises()
    expect(clipboard.dialog).toMatchObject({ mode: 'copy', text: ' 中文🙂\n' })
    state.generation++; expect(clipboard.dialog.mode).toBe('')
  })
  it('toolbar falls back to a paste dialog without Clipboard API; Android uses clipboard paste and browser inserts text', async () => {
    vi.stubGlobal('navigator', {})
    const store = reactive({ deviceId: 'phone' }), fallback = vi.fn(), sendControl = vi.fn(() => true)
    const scope = effectScope(); scopes.push(scope)
    const manager = scope.run(() => useConsoleDeviceManager({ store, devicesData: ref([]), connected: ref(true),
      consoleRuntime: { scanning: ref(false) }, toast: vi.fn(), sendControl, requestPasteText: fallback }))
    await manager.clipboard(); expect(fallback).toHaveBeenCalledWith('phone')
    expect(manager.pasteText(' 中文🙂\n')).toBe(true)
    expect(sendControl).toHaveBeenLastCalledWith({ type: 'clipboard', text: ' 中文🙂\n', paste: true })
    store.deviceId = 'browser-one'
    expect(manager.pasteText('网页文字')).toBe(true)
    expect(sendControl).toHaveBeenLastCalledWith({ type: 'text', text: '网页文字' })
    expect(manager.pasteText('旧目标文本', 'phone')).toBe(false)
    expect(manager.pasteText('中'.repeat(6000))).toBe(false)
  })
  it('paste dialog keeps native editing and requires an explicit send', async () => {
    const state = reactive({ mode: '', text: '', error: '' })
    const wrapper = mount(ConsoleClipboardDialog, { props: { state }, global: { stubs: { Teleport: true }, directives: { 'backdrop-dismiss': {} } } })
    state.mode = 'paste'; await flushPromises()
    await wrapper.get('textarea').setValue(' 原文🙂\n')
    expect(wrapper.emitted('submit')).toBeUndefined()
    await wrapper.get('form').trigger('submit')
    expect(wrapper.emitted('submit')[0]).toEqual([' 原文🙂\n'])
    wrapper.unmount()
  })
})
