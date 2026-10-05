import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { effectScope, reactive, ref } from 'vue'
import { useConsoleInputControl } from './components/console/useConsoleInputControl'
import { useConsoleDeviceManager } from './components/console/useConsoleDeviceManager'
import { api } from './api'
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => vi.fn(async () => true) }))

let scopes = []
const settle = async () => { for (let i = 0; i < 6; i++) await Promise.resolve() }
function setup(load, id = ref('phone')) {
  const scope = effectScope(); scopes.push(scope)
  const toast = vi.fn()
  const control = scope.run(() => useConsoleInputControl({ deviceId: id, load, toast }))
  return { control, id, toast, scope }
}
const state = (phase, manual_allowed, generation = 1) => ({ phase, manual_allowed, owner: 'session', generation })
beforeEach(() => { vi.useFakeTimers() })
afterEach(() => { scopes.forEach(scope => scope.stop()); scopes = []; vi.useRealTimers(); vi.unstubAllGlobals() })

it('在权威running/pausing/resuming/stopping状态拒绝输入，paused才放行', async () => {
  let current = state('running', false)
  const { control, toast } = setup(vi.fn(async () => current)); await settle()
  for (const phase of ['running', 'pausing', 'resuming', 'stopping']) {
    current = state(phase, false)
    await control.refresh()
    for (const type of ['touch', 'key', 'input_event', 'text', 'tap', 'scroll', 'start_app', 'stop_app', 'rotate']) {
      expect(control.guardDeviceInput({ type })).toBe(false)
    }
  }
  expect(toast).toHaveBeenCalledTimes(4)
  expect(control.guardDeviceInput({ type: 'audio' })).toBe(true)
  expect(control.guardDeviceInput({ type: 'reset_video' })).toBe(true)
  current = state('paused', true, 2); await control.refresh()
  expect(control.manualAllowed.value).toBe(true)
  expect(control.message.value).toContain('可以人工操作')
  expect(control.guardDeviceInput({ type: 'touch' })).toBe(true)
})

it('切换目标立即失效旧状态，迟到的旧目标响应不能放行新目标输入', async () => {
  const resolves = new Map()
  const load = vi.fn((id) => new Promise(resolve => resolves.set(id, resolve)))
  const { control, id } = setup(load)
  expect(control.manualAllowed.value).toBe(false)
  id.value = 'browser-one'
  resolves.get('phone')(state('idle', true)); await settle()
  expect(control.manualAllowed.value).toBe(false)
  resolves.get('browser-one')(state('running', false)); await settle()
  expect(control.status.value.phase).toBe('running')
  expect(control.manualAllowed.value).toBe(false)
  expect(load).toHaveBeenCalledTimes(2)
})

it('读取失败和无效响应关闭人工输入；恢复读取后允许idle，轮询随scope清理', async () => {
  const load = vi.fn().mockResolvedValue(state('idle', true))
  const { control, scope } = setup(load); await settle()
  expect(control.manualAllowed.value).toBe(true)
  load.mockRejectedValueOnce(new Error('网络中断')); await control.refresh()
  expect(control.manualAllowed.value).toBe(false)
  expect(control.message.value).toContain('无法同步')
  load.mockResolvedValueOnce({ phase: 'paused' }); await control.refresh()
  expect(control.manualAllowed.value).toBe(false)
  await control.refresh()
  expect(control.manualAllowed.value).toBe(true)
  scope.stop()
  const count = load.mock.calls.length
  await vi.advanceTimersByTimeAsync(10000)
  expect(load).toHaveBeenCalledTimes(count)
})

it('设备状态轮询使用编码后的通用REST入口，并传递取消信号', async () => {
  const fetch = vi.fn(async () => ({ ok: true, headers: { get: () => 'application/json' }, json: async () => state('paused', true) }))
  vi.stubGlobal('fetch', fetch)
  const abort = new AbortController()
  await api.getInputControl('browser-test/one', { signal: abort.signal })
  expect(fetch).toHaveBeenCalledWith('/api/devices/browser-test%2Fone/input-control', expect.objectContaining({ method: 'GET', signal: abort.signal }))
})

it('人工应用启动/停止与粘贴都经过同一门禁；等待剪贴板期间收回控制权不发送文字', async () => {
  let allowed = false, resolveClipboard
  const readText = vi.fn(() => new Promise(resolve => { resolveClipboard = resolve }))
  vi.stubGlobal('navigator', { clipboard: { readText } })
  const scope = effectScope(); scopes.push(scope)
  const sendControl = vi.fn(() => true), toast = vi.fn()
  const manager = scope.run(() => useConsoleDeviceManager({
    store: reactive({ deviceId: 'phone' }), devicesData: ref([{ id: 'phone', name: '手机', pkg: 'com.game' }]),
    connected: ref(true), errorMsg: ref(''), appHintDismissed: ref(false),
    consoleRuntime: { scanning: ref(false) }, toast, sendControl,
    guardManualInput: () => allowed,
  }))
  manager.launchGame(); manager.stopGame(); await manager.clipboard()
  expect(sendControl).not.toHaveBeenCalled(); expect(readText).not.toHaveBeenCalled()
  allowed = true; manager.launchGame(); manager.stopGame()
  expect(sendControl.mock.calls.map(call => call[0].type)).toEqual(['start_app', 'stop_app'])
  sendControl.mockClear(); toast.mockClear()
  const pending = manager.clipboard(); await settle()
  allowed = false; resolveClipboard('不会发送的文本'); await pending
  expect(sendControl).not.toHaveBeenCalled()
  expect(toast.mock.calls.some(call => call[0].includes('已粘贴'))).toBe(false)
})
