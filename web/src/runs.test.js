// 当前 RUN-003 契约：运行实例只用 run_id，设备活动查询只接受 {active,run}。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

function res(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: h => (/^content-type$/i.test(h) ? 'application/json' : null) },
    json: async () => body,
  }
}

let runs, storeMod, api

async function fresh() {
  vi.resetModules()
  runs = await import('../src/runs')
  storeMod = await import('../src/store')
  ;({ api } = await import('../src/api'))
}

beforeEach(async () => {
  vi.stubGlobal('fetch', vi.fn())
  vi.stubGlobal('location', { hash: '#/console' })
  await fresh()
})

afterEach(() => vi.unstubAllGlobals())

describe('运行语义工具', () => {
  it('保留当前状态集合、来源标签和 run_id 展示格式', () => {
    expect(runs.isActiveRunState('starting')).toBe(true)
    expect(runs.isTerminalRunState('cancelled')).toBe(true)
    expect(runs.isActiveRunState('running_old')).toBe(false)
    expect(runs.sourceLabel('manual')).toBe('手动')
    expect(runs.sourceLabel('scheduled')).toBe('定时')
    expect(runs.sourceLabel('task_now')).toBe('手动任务')
    expect(runs.shortRunId('550e8400-e29b-41d4-a716-446655440000')).toBe('550e8400')
  })

  it('冲突文案读取当前 device_busy 的 run_id 相关字段', () => {
    const busy = {
      error: 'device_busy',
      run_id: 'r-1',
      script_id: 'hkrpg/daily.yaml',
      source: 'scheduled',
      started_at: '2026-08-27T07:30:05Z',
    }
    expect(runs.describeConflict(busy)).toContain('hkrpg/daily.yaml')
    expect(runs.describeConflict(busy)).toContain('来源：定时')
    expect(runs.isDeviceBusyConflict({ status: 409, data: busy })).toBe(true)
    expect(runs.isDeviceBusyConflict({ status: 404, data: busy })).toBe(false)
  })
})

describe('当前运行查询与启动响应', () => {
  it('设备活动响应保留当前 envelope，不再读取旧 running 形状', async () => {
    fetch.mockResolvedValueOnce(res(200, {
      active: true,
      run: { run_id: 'rid-1', device_id: 'dev-1', script_id: 'p/login.yaml', state: 'running' },
    }))
    await expect(api.deviceRun('dev-1')).resolves.toEqual({
      active: true,
      run: { run_id: 'rid-1', device_id: 'dev-1', script_id: 'p/login.yaml', state: 'running' },
    })
    expect(fetch).toHaveBeenCalledWith('/api/devices/dev-1/run', expect.objectContaining({ method: 'GET' }))

    fetch.mockResolvedValueOnce(res(200, { running: true, script_id: 'old.yaml' }))
    await expect(api.deviceRun('dev-1')).rejects.toMatchObject({ code: 'invalid_response', status: 502 })
  })

  it('启动、查询和任务立即运行都要求服务端返回 run_id', async () => {
    fetch.mockResolvedValueOnce(res(202, { run_id: 'run-fast', state: 'starting', resolved_args: {} }))
    await expect(api.run({ runner_id: 'test.runner', entrypoint: 'p/fast.yaml', device_id: 'dev-1' })).resolves.toMatchObject({ run_id: 'run-fast' })

    fetch.mockResolvedValueOnce(res(200, { run_id: 'run-fast', state: 'success' }))
    await expect(api.getRun('run-fast')).resolves.toMatchObject({ run_id: 'run-fast', state: 'success' })

    fetch.mockResolvedValueOnce(res(202, { run_id: 'task-run' }))
    await expect(api.runTaskNow('task-1')).resolves.toEqual({ run_id: 'task-run' })

    fetch.mockResolvedValueOnce(res(202, { ok: true }))
    await expect(api.run({ runner_id: 'test.runner', entrypoint: 'p/fast.yaml', device_id: 'dev-1' })).rejects.toMatchObject({
      code: 'invalid_response',
      status: 502,
      data: { ok: true },
    })
  })

  it('取消只按 run_id 寻址并保留当前错误对象', async () => {
    fetch.mockResolvedValueOnce(res(202, { cancelling: true }))
    await expect(api.cancelRun('550e8400')).resolves.toEqual({ cancelling: true })
    expect(fetch).toHaveBeenCalledWith('/api/runs/550e8400/cancel', expect.objectContaining({ method: 'POST' }))

    fetch.mockRejectedValueOnce(new TypeError('offline'))
    await expect(api.cancelRun('550e8400')).rejects.toMatchObject({
      name: 'ApiError', status: 0, code: 'network_error', data: null,
    })
  })
})

describe('运行注册表只接受 run_id 和当前状态', () => {
  it('current record 驱动活动反查，终态清理并拒收非法状态', () => {
    storeMod.store.deviceId = 'dev-2'
    expect(storeMod.applyRunRecord({ script_id: 'p/x.yaml', state: 'running' })).toBeNull()
    storeMod.applyRunRecord({ run_id: 'run-2', device_id: 'dev-2', script_id: 'p/x.yaml', state: 'running' })
    expect(storeMod.store.runId).toBe('run-2')
    expect(storeMod.getActiveRun('dev-2').run_id).toBe('run-2')

    storeMod.applyRunRecord({ run_id: 'run-2', device_id: 'dev-2', script_id: 'p/x.yaml', state: 'success' })
    expect(storeMod.getActiveRun('dev-2')).toBeNull()
    expect(storeMod.store.running).toBe(false)
  })
})

describe('时间格式化', () => {
  it('ISO 时间转本地秒级文本，非法输入原样返回', () => {
    const iso = '2026-08-27T07:30:05Z'
    const d = new Date(iso)
    const p = n => String(n).padStart(2, '0')
    expect(runs.formatLocalTime(iso)).toBe(
      `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`,
    )
    expect(runs.formatLocalTime('昨天早上')).toBe('昨天早上')
  })
})
