import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { useConsoleRuntime } from './composables/useConsoleRuntime'

function makeApi(overrides = {}) {
  return {
    listDevices: vi.fn().mockResolvedValue([{ id: 'dev-a' }, { id: 'dev-b' }]),
    scanDevices: vi.fn().mockResolvedValue({ added: 1, devices: [{ id: 'dev-a' }, { id: 'dev-c' }] }),
    listLogs: vi.fn().mockResolvedValue([{ time: '2026-08-28 10:00:00', level: 'info', msg: 'hi' }]),
    ...overrides,
  }
}

describe('useConsoleRuntime', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  it('只拉设备（壳不预取脚本/模板等业务资源）并支持扫描刷新', async () => {
    const api = makeApi()
    const runtime = useConsoleRuntime({
      api,
      devicesData: ref([]),
      deviceIdRef: ref('dev-z'),
    })

    await runtime.loadData()
    expect(api.listDevices).toHaveBeenCalled()
    expect(api.listScripts).toBeUndefined()
    expect(api.listTemplates).toBeUndefined()

    const rep = await runtime.refreshDevices()
    expect(rep.ok).toBe(true)
    expect(rep.added).toBe(1)
    expect(runtime.scanning.value).toBe(false)
  })

  it('starts log polling and stops it through cleanup', async () => {
    const api = makeApi({ listLogs: vi.fn().mockResolvedValue([]) })
    const runtime = useConsoleRuntime({
      api,
      devicesData: ref([]),
      deviceIdRef: ref('dev-a'),
    })
    const onTick = vi.fn()
    runtime.startLogPolling(onTick)
    expect(onTick).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(1000)
    expect(onTick).toHaveBeenCalledTimes(2)
    runtime.cleanup()
    runtime.cleanup()
    vi.advanceTimersByTime(2000)
    expect(onTick).toHaveBeenCalledTimes(2)
    expect(runtime.reconnectTimer.value).toBeNull()
  })
})
