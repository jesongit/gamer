import { beforeEach, describe, expect, it, vi } from 'vitest'

let storeMod

beforeEach(async () => {
  vi.resetModules()
  storeMod = await import('./store')
  storeMod.store.deviceId = null
  storeMod.resetStoreRunState()
})

describe('store run registry', () => {
  it('公开状态只保留 runId，不暴露 script_id 执行句柄', () => {
    expect(storeMod.store).not.toHaveProperty('runScriptId')
    expect(storeMod.store.runId).toBeNull()
  })

  it('beginCancel 只接收 run_id 并先迁移到 stopping', () => {
    storeMod.store.deviceId = 'dev-1'
    storeMod.applyRunRecord({ run_id: 'run-1', device_id: 'dev-1', script_id: 'p/main.yaml', state: 'running' })
    expect(storeMod.beginCancel('run-1')).toMatchObject({ run_id: 'run-1', state: 'stopping' })
    expect(storeMod.getActiveRun('dev-1')).toMatchObject({ run_id: 'run-1', state: 'stopping' })
    expect(storeMod.store.running).toBe(true)
    expect(storeMod.store.runStep).toBe('正在停止…')
  })

  it('终态清除活动反查和当前展示，迟到活动记录不能复活', () => {
    storeMod.store.deviceId = 'dev-2'
    storeMod.applyRunRecord({ run_id: 'run-2', device_id: 'dev-2', script_id: 'p/main.yaml', state: 'running' })
    storeMod.applyRunRecord({ run_id: 'run-2', device_id: 'dev-2', script_id: 'p/main.yaml', state: 'cancelled' })
    expect(storeMod.runRegistry.last).toMatchObject({ run_id: 'run-2', state: 'cancelled' })
    expect(storeMod.getActiveRun('dev-2')).toBeNull()
    expect(storeMod.store.runId).toBeNull()
    expect(storeMod.store.running).toBe(false)

    expect(storeMod.applyRunRecord({ run_id: 'run-2', device_id: 'dev-2', script_id: 'p/main.yaml', state: 'running' }))
      .toMatchObject({ state: 'cancelled' })
    expect(storeMod.store.running).toBe(false)
  })
})
