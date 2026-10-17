import { describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, reactive } from 'vue'
import { useRunningPackageContext } from './composables/useRunningPackageContext'

function setup() {
  const state = reactive({
    device: 'd1', current: 'default', loaded: true, editable: true,
    packages: [{ id: 'default' }, { id: 'hsr' }, { id: 'other' }],
    runs: {},
  })
  const select = vi.fn(id => { state.current = id; return true })
  const scope = effectScope()
  scope.run(() => useRunningPackageContext({
    deviceId: () => state.device,
    activeRun: () => state.runs[state.device],
    packages: () => state.packages,
    loaded: () => state.loaded,
    currentPackageId: () => state.current,
    selectPackage: select,
    canSwitch: () => state.editable,
  }))
  return { state, select, scope }
}

describe('running package context', () => {
  it('follows the selected device and an opaque runner entrypoint using explicit package context', async () => {
    const { state, select, scope } = setup()
    state.runs.d1 = { run_id: 'r1', device_id: 'd1', content_package: 'hsr', entrypoint: 'opaque' }
    state.runs.d2 = { run_id: 'r2', device_id: 'd2', content_package: 'other' }
    await nextTick()
    expect(state.current).toBe('hsr')
    state.current = 'default' // explicit browsing stays respected while the run is unchanged
    state.runs.d1.state = 'running'
    await nextTick()
    expect(state.current).toBe('default')
    state.device = 'd2'
    await nextTick()
    expect(state.current).toBe('other')
    expect(select.mock.calls.map(c => c[0])).toEqual(['hsr', 'other'])
    scope.stop()
  })

  it('waits for loaded packages and for unsaved editing to clear', async () => {
    const { state, select, scope } = setup()
    state.loaded = false
    state.editable = false
    state.runs.d1 = { run_id: 'r1', device_id: 'd1', content_package: 'hsr' }
    await nextTick()
    state.loaded = true
    await nextTick()
    expect(select).not.toHaveBeenCalled()
    state.editable = true
    await nextTick()
    expect(state.current).toBe('hsr')
    scope.stop()
  })

  it('ignores unrelated, missing or unavailable context instead of inferring from application or entrypoint', async () => {
    const { state, select, scope } = setup()
    state.runs.d1 = { run_id: 'r1', device_id: 'd2', content_package: 'hsr' }
    await nextTick()
    state.runs.d1 = { run_id: 'r2', device_id: 'd1', android_package: 'hsr', entrypoint: 'hsr/guess' }
    await nextTick()
    state.runs.d1 = { run_id: 'r3', device_id: 'd1', content_package: 'removed' }
    await nextTick()
    expect(select).not.toHaveBeenCalled()
    scope.stop()
  })
})
