import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
const { dialogDecision } = vi.hoisted(() => ({ dialogDecision: vi.fn() }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => Object.assign(dialogDecision, { cancel: vi.fn() }) }))
import { effectScope, reactive, ref } from 'vue'
import { useConsoleDeviceManager } from './components/console/useConsoleDeviceManager'
import { api } from './api'

vi.mock('./api', () => ({ api: { forceReconnectDevice: vi.fn(), listDevices: vi.fn() } }))

let scope
function setup() {
  const events = []
  const options = {
    toast: vi.fn(), store: reactive({ deviceId: 'target' }),
    devicesData: ref([{ id: 'target', name: '平板' }]),
    consoleRuntime: { scanning: ref(false), cancelReconnect: vi.fn(() => events.push('cancel')) },
    connected: ref(false), errorMsg: ref('旧错误'), appHintDismissed: ref(false),
    loadData: vi.fn(), connect: vi.fn(async () => events.push('connect')),
    cleanup: vi.fn(() => events.push('cleanup')), sendControl: vi.fn(),
  }
  scope = effectScope()
  const manager = scope.run(() => useConsoleDeviceManager(options))
  return { ...options, manager, events }
}

beforeEach(() => {
  vi.resetAllMocks()
  dialogDecision.mockResolvedValue(true)
  vi.stubGlobal('localStorage', { setItem: vi.fn() })
  api.listDevices.mockResolvedValue([{ id: 'target', name: '平板', status: 'online' }])
})
afterEach(() => { scope?.stop(); vi.unstubAllGlobals() })

describe('设备强制重连', () => {
  it('取消时不拆连接或调用后端', async () => {
    dialogDecision.mockResolvedValue(false)
    const ctx = setup()
    await ctx.manager.forceReconnect()
    expect(ctx.cleanup).not.toHaveBeenCalled()
    expect(api.forceReconnectDevice).not.toHaveBeenCalled()
  })

  it('先停止自动重连，等待后端恢复后再建立投屏，阻止重复提交', async () => {
    const ctx = setup()
    let finish
    api.forceReconnectDevice.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const pending = ctx.manager.forceReconnect()
    await Promise.resolve()
    expect(ctx.manager.forceReconnecting.value).toBe(true)
    expect(ctx.events).toEqual(['cancel', 'cleanup'])
    await ctx.manager.forceReconnect()
    await ctx.manager.flushAndConnect()
    expect(api.forceReconnectDevice).toHaveBeenCalledTimes(1)
    expect(ctx.connect).not.toHaveBeenCalled()
    finish({ ok: true })
    await pending
    expect(ctx.events).toEqual(['cancel', 'cleanup', 'connect'])
    expect(ctx.errorMsg.value).toBe('')
    expect(ctx.manager.forceReconnecting.value).toBe(false)
  })

  it('恢复失败显示原因，恢复按钮可再次使用', async () => {
    api.forceReconnectDevice.mockRejectedValue(new Error('请先停止任务'))
    const ctx = setup()
    await ctx.manager.forceReconnect()
    expect(ctx.errorMsg.value).toContain('请先停止任务')
    expect(ctx.connect).not.toHaveBeenCalled()
    expect(ctx.manager.forceReconnecting.value).toBe(false)
  })

  it('恢复过程中切换设备不会连接错误的设备', async () => {
    const ctx = setup()
    api.forceReconnectDevice.mockImplementation(async () => { ctx.store.deviceId = 'other' })
    await ctx.manager.forceReconnect()
    expect(ctx.connect).not.toHaveBeenCalled()
  })
})
