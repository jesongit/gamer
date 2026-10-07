// Independent integration checks: real workspace controller + reactive runtime registry.
// Transport is mocked; these are not browser/real-device performance measurements.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import { createMultiviewWorkspace, MULTIVIEW_STORAGE_KEY } from './console/multiview-state.mjs'
import { applyRunRecord, beginCancel, getActiveRun, resetStoreRunState, runRegistry, store } from './store'

const targets = ['android-a', 'browser-b', 'android-c', 'browser-d', 'android-e', 'browser-f', 'android-g', 'browser-h', 'android-i']
const record = (id, runId = `run-${id}`, state = 'running') => ({ device_id: id, run_id: runId, state, runner_id: 'gamer-yaml', entrypoint: 'daily/main.yaml' })
function setup(saved) {
  const values = new Map(saved ? [[MULTIVIEW_STORAGE_KEY, JSON.stringify(saved)]] : [])
  const storage = { getItem: key => values.get(key), setItem: (key, value) => values.set(key, value) }
  const api = {
    deviceRun: vi.fn(async () => ({ active: false })),
    getRun: vi.fn(async runId => ({ ...runRegistry.byId[runId], state: 'success' })),
    run: vi.fn(async input => ({ run_id: `new-${input.device_id}`, state: 'starting' })),
    cancelRun: vi.fn(async () => ({ cancelling: true })),
  }
  const create = () => createMultiviewWorkspace({ reactive, storage, api, registry: runRegistry, applyRunRecord, beginCancel })
  return { workspace: create(), api, create, storage }
}
beforeEach(() => {
  runRegistry.byId = {}; runRegistry.activeByDevice = {}; runRegistry.last = null
  store.deviceId = null; resetStoreRunState()
})

describe('multiview runtime/workspace integration', () => {
  it('keeps target package/script/args separate and restores layout without launching runs', async () => {
    const { workspace, api, create } = setup()
    expect(workspace.state.gridSize).toBe(4)
    for (const id of targets) expect(workspace.addTarget(id)).toBe(true)
    expect(workspace.addTarget(targets[0])).toBe(false)
    expect(workspace.addTarget('browser-tenth')).toBe(false)
    workspace.updateConfig(targets[0], { packageId: 'daily', scriptId: 'daily/one.yaml', runnerId: 'gamer-yaml', args: { account: 'one' } })
    workspace.updateConfig(targets[1], { packageId: 'other', scriptId: 'other/two.yaml', runnerId: 'gamer-yaml', args: { account: 'two' } })
    await workspace.selectTarget(targets[1])
    const restored = create()
    expect(restored.state.selectedTargetId).toBe(targets[1])
    expect(restored.getConfig(targets[0]).args).toEqual({ account: 'one' })
    expect(restored.getConfig(targets[1]).args).toEqual({ account: 'two' })
    expect(api.run).not.toHaveBeenCalled()
    api.deviceRun.mockImplementation(async id => ({ active: true, run: record(id) }))
    await restored.refreshRunStates()
    expect(getActiveRun(targets[8]).run_id).toBe(`run-${targets[8]}`)
    expect(api.run).not.toHaveBeenCalled()
  })

  it('selection veto leaves layout, current runtime projection and target config intact', async () => {
    const { workspace } = setup()
    targets.forEach(id => workspace.addTarget(id))
    await workspace.setGridSize(9)
    await workspace.selectTarget(targets[8])
    store.deviceId = targets[8]
    applyRunRecord(record(targets[8]))
    const project = vi.fn()
    const accepted = await workspace.setGridSize(4, { beforeChange: async () => false, project })
    expect(accepted).toBe(false)
    expect(project).not.toHaveBeenCalled()
    expect(workspace.state.gridSize).toBe(9)
    expect(workspace.state.selectedTargetId).toBe(targets[8])
    expect(store.runId).toBe(`run-${targets[8]}`)
  })

  it('hiding or removing previews preserves every background runtime', async () => {
    const { workspace, api } = setup()
    targets.forEach(id => { workspace.addTarget(id); applyRunRecord(record(id)) })
    await workspace.setGridSize(9)
    await workspace.selectTarget(targets[8])
    await workspace.setGridSize(4)
    await workspace.removeTarget(targets[0])
    expect(api.cancelRun).not.toHaveBeenCalled()
    for (const id of targets) expect(getActiveRun(id)?.state).toBe('running')
  })

  it('batch stop excludes runs replaced after confirmation snapshot and currently hidden targets', async () => {
    const { workspace, api } = setup()
    targets.forEach(id => { workspace.addTarget(id); applyRunRecord(record(id)) })
    await workspace.setGridSize(9)
    const snapshot = workspace.captureVisibleStops()
    applyRunRecord(record(targets[0], 'replacement-run'))
    await workspace.setGridSize(4)
    await workspace.stopCaptured(snapshot)
    expect(api.cancelRun.mock.calls.map(([id]) => id).sort()).toEqual(targets.slice(1, 4).map(id => `run-${id}`).sort())
    expect(getActiveRun(targets[0]).run_id).toBe('replacement-run')
    expect(getActiveRun(targets[0]).state).toBe('running')
    for (const id of targets.slice(4)) expect(getActiveRun(id).state).toBe('running')
  })

  it('batch start uses each visible target configuration and skips existing activity', async () => {
    const { workspace, api } = setup()
    targets.forEach((id, index) => {
      workspace.addTarget(id)
      workspace.updateConfig(id, { packageId: `package-${index}`, scriptId: `package-${index}/main.yaml`, runnerId: 'gamer-yaml', args: { index } })
    })
    applyRunRecord(record(targets[1]))
    const results = await workspace.startVisible()
    expect(results.map(result => result.status)).toEqual(['started', 'skipped', 'started', 'started'])
    expect(api.run.mock.calls.map(([input]) => input)).toEqual([0, 2, 3].map(index => ({
      device_id: targets[index], content_package: `package-${index}`, runner_id: 'gamer-yaml', entrypoint: `package-${index}/main.yaml`, payload: { args: { index } },
    })))
    expect(getActiveRun(targets[1]).run_id).toBe(`run-${targets[1]}`)
    expect(getActiveRun(targets[4])).toBeNull()
  })

  it('an in-flight recovery response cannot replace a newer locally registered run', async () => {
    const { workspace, api } = setup()
    workspace.addTarget(targets[0])
    applyRunRecord(record(targets[0], 'old-run'))
    let finish
    api.deviceRun.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const recovery = workspace.refreshRunStates()
    applyRunRecord(record(targets[0], 'newer-run', 'starting'))
    finish({ active: true, run: record(targets[0], 'old-run') })
    await recovery
    expect(getActiveRun(targets[0]).run_id).toBe('newer-run')
  })
  it('fallback run-detail recovery cannot reclaim a target from a newer run', async () => {
    const { workspace, api } = setup()
    workspace.addTarget(targets[0])
    applyRunRecord(record(targets[0], 'old-run'))
    let finish
    api.getRun.mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const recovery = workspace.refreshRunStates()
    await Promise.resolve()
    applyRunRecord(record(targets[0], 'newer-run', 'starting'))
    finish(record(targets[0], 'old-run'))
    await recovery
    expect(getActiveRun(targets[0]).run_id).toBe('newer-run')
  })

})
