import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { useConsoleRuntime } from './composables/useConsoleRuntime'

function makeApi(overrides = {}) {
  return {
    listDevices: vi.fn().mockResolvedValue([{ id: 'dev-a' }, { id: 'dev-b' }]),
    listScripts: vi.fn().mockResolvedValue([{ id: 's-1' }]),
    listTemplates: vi.fn().mockResolvedValue([{ name: 'tpl-1' }]),
    scanDevices: vi.fn().mockResolvedValue({ added: 1, devices: [{ id: 'dev-a' }, { id: 'dev-c' }] }),
    listLogs: vi.fn().mockResolvedValue([{ time: '2026-08-28 10:00:00', level: 'info', msg: 'hi' }]),
    ...overrides,
  }
}

describe('useConsoleRuntime', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  it('loads resources and refreshes devices', async () => {
    const api = makeApi()
    const runtime = useConsoleRuntime({
      api,
      devicesData: ref([]),
      scriptsData: ref([]),
      templatesData: ref([]),
      toast: vi.fn(),
      connect: vi.fn(),
      deviceIdRef: ref('dev-z'),
    })

    await runtime.loadData()
    expect(api.listDevices).toHaveBeenCalled()
    expect(api.listScripts).toHaveBeenCalled()
    expect(api.listTemplates).toHaveBeenCalled()

    const rep = await runtime.refreshDevices()
    expect(rep.ok).toBe(true)
    expect(rep.added).toBe(1)
    expect(runtime.scanning.value).toBe(false)
  })

  it('schedules reconnect, cancels it, and cleans up idempotently', () => {
    const toast = vi.fn()
    const connect = vi.fn()
    const runtime = useConsoleRuntime({
      api: makeApi(),
      devicesData: ref([]),
      scriptsData: ref([]),
      templatesData: ref([]),
      toast,
      connect,
      deviceIdRef: ref('dev-a'),
    })

    const superseded = ref(false)
    const errorMsg = ref('')
    expect(runtime.scheduleReconnect({ superseded, errorMsg })).toBe(true)
    expect(toast).toHaveBeenCalledWith('连接已断开，3 秒后自动重连…', 'warn')
    vi.advanceTimersByTime(3000)
    expect(connect).toHaveBeenCalledWith(false)
    runtime.cleanup()
    runtime.cleanup()
    expect(runtime.reconnectTimer.value).toBeNull()
  })

  it('starts log polling and stops it through cleanup', async () => {
    const api = makeApi({ listLogs: vi.fn().mockResolvedValue([]) })
    const runtime = useConsoleRuntime({
      api,
      devicesData: ref([]),
      scriptsData: ref([]),
      templatesData: ref([]),
      toast: vi.fn(),
      connect: vi.fn(),
      deviceIdRef: ref('dev-a'),
    })
    const onTick = vi.fn()
    runtime.startLogPolling(onTick)
    expect(onTick).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(1000)
    expect(onTick).toHaveBeenCalledTimes(2)
    runtime.cleanup()
    vi.advanceTimersByTime(2000)
    expect(onTick).toHaveBeenCalledTimes(2)
  })
})
