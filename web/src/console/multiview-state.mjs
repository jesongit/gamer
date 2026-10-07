// Layout/configuration only. Runtime truth remains in the existing run registry/API.
export const MULTIVIEW_STORAGE_KEY = 'gamer.console.multiview.v1'
const clone = value => JSON.parse(JSON.stringify(value))
const emptyConfig = () => ({ packageId: '', scriptId: '', runnerId: '', args: {} })
const own = (object, key) => object && Object.hasOwn(object, key) ? object[key] : undefined
const cleanString = value => typeof value === 'string' ? value.trim() : ''
function cleanConfig(value) {
  return {
    packageId: cleanString(value?.packageId), scriptId: cleanString(value?.scriptId), runnerId: cleanString(value?.runnerId),
    args: value?.args && typeof value.args === 'object' && !Array.isArray(value.args) ? clone(value.args) : {},
  }
}
export function createMultiviewWorkspace({ reactive = value => value, storage, api, registry, applyRunRecord, beginCancel, isOnline = () => true } = {}) {
  let saved
  try { saved = JSON.parse(storage?.getItem(MULTIVIEW_STORAGE_KEY) || 'null') } catch {}
  const targetIds = [...new Set((Array.isArray(saved?.targetIds) ? saved.targetIds : []).filter(id => typeof id === 'string' && id))].slice(0, 9)
  const state = reactive({ gridSize: saved?.gridSize === 9 ? 9 : 4, targetIds, selectedTargetId: null, configs: Object.create(null), pending: Object.create(null), errors: Object.create(null) })
  // Removing a preview does not forget its configuration. Restore remembered targets
  // separately from layout membership; only explicit addTarget brings them back.
  if (saved?.configs && typeof saved.configs === 'object' && !Array.isArray(saved.configs)) {
    for (const [id, config] of Object.entries(saved.configs)) {
      if (id && config && typeof config === 'object' && !Array.isArray(config)) state.configs[id] = cleanConfig(config)
    }
  }
  for (const id of targetIds) state.configs[id] ||= emptyConfig()
  state.selectedTargetId = targetIds.slice(0, state.gridSize).includes(saved?.selectedTargetId) ? saved.selectedTargetId : targetIds[0] || null
  let timer = null, refreshing = null, selecting = false
  const visible = () => state.targetIds.slice(0, state.gridSize)
  const persist = () => { try { storage?.setItem(MULTIVIEW_STORAGE_KEY, JSON.stringify({ gridSize: state.gridSize, targetIds: state.targetIds, selectedTargetId: state.selectedTargetId, configs: state.configs })) } catch {} }
  function getConfig(id) { return own(state.configs, id) || null }
  function updateConfig(id, patch) {
    if (!state.targetIds.includes(id)) return false
    const old = getConfig(id) || emptyConfig()
    state.configs[id] = cleanConfig({ ...old, ...(patch?.packageId !== undefined && patch.packageId !== old.packageId ? { scriptId: '', args: {} } : {}), ...patch })
    persist(); return true
  }
  function addTarget(id) {
    if (typeof id !== 'string' || !id || state.targetIds.includes(id) || state.targetIds.length >= 9) return false
    state.targetIds.push(id); state.configs[id] ||= emptyConfig(); persist(); return true
  }
  async function selectTarget(id, { beforeChange = async () => true, project = async () => true } = {}) {
    if (id !== null && !visible().includes(id)) return false
    if (id === state.selectedTargetId) return true
    if (selecting) return false
    selecting = true
    const previous = state.selectedTargetId
    try {
      if (!await beforeChange(id, previous) || !await project(id, previous)) return false
      state.selectedTargetId = id; persist(); return true
    } finally { selecting = false }
  }
  async function removeTarget(id, options) {
    if (selecting) return false
    if (!state.targetIds.includes(id)) return false
    if (state.selectedTargetId === id && !await selectTarget(visible().find(candidate => candidate !== id) || null, options)) return false
    state.targetIds = state.targetIds.filter(candidate => candidate !== id)
    // Keep its configuration during this session; removing a tile never cancels its run.
    persist(); return true
  }
  async function setGridSize(size, options) {
    if (selecting) return false
    if (size !== 4 && size !== 9) return false
    if (size === 4 && state.targetIds.indexOf(state.selectedTargetId) >= 4 && !await selectTarget(state.targetIds[0] || null, options)) return false
    state.gridSize = size; persist(); return true
  }
  function getRun(id) { return own(registry.byId, own(registry.activeByDevice, id)) || null }
  async function refreshRunStates(extraTargetIds = []) {
    if (refreshing) return refreshing
    refreshing = (async () => {
      // Poll all known active instances, including hidden/removed targets. Recover every tile.
      const ids = [...new Set([...state.targetIds, ...extraTargetIds, ...Object.keys(registry.activeByDevice)])]
      await Promise.all(ids.map(async id => {
        try {
          const previous = getRun(id)?.run_id
          const result = await api.deviceRun(id)
          if (result?.active && result.run?.run_id) {
            const current = getRun(id)?.run_id
            if (current === previous || current === result.run.run_id) applyRunRecord({ ...result.run, device_id: id })
          }
          else if (previous) {
            const record = await api.getRun(previous)
            if (getRun(id)?.run_id === previous || ['success', 'failed', 'cancelled'].includes(record?.state)) applyRunRecord(record)
          }
          delete state.errors[id]
        } catch (e) { state.errors[id] = e.message || String(e) }
      }))
    })().finally(() => { refreshing = null })
    return refreshing
  }
  function startPolling() { if (!timer) { void refreshRunStates(); timer = setInterval(() => void refreshRunStates(), 1000) } }
  function stopPolling() { if (timer) clearInterval(timer); timer = null }
  async function startConfiguredTarget(id, config) {
    const result = (status, reason) => ({ targetId: id, status, reason })
    if (!state.targetIds.includes(id)) return result('skipped', '目标已移除')
    if (state.pending[id] || getRun(id)) return result('skipped', '目标已有运行')
    if (!isOnline(id)) return result('skipped', '目标离线')
    if (!config.packageId || !config.scriptId || !config.runnerId) return result('skipped', '配置包或脚本未选择')
    state.pending[id] = true
    try {
      // Final authority is the existing run endpoint; no queue or separate run system.
      const rep = await api.run({ device_id: id, content_package: config.packageId, runner_id: config.runnerId, entrypoint: config.scriptId, payload: { args: config.args || {} } })
      applyRunRecord({ ...rep, device_id: id, entrypoint: config.scriptId, runner_id: config.runnerId, source: 'manual' })
      return { ...result('started'), runId: rep.run_id }
    } catch (e) { state.errors[id] = e.message || String(e); return result('error', state.errors[id]) }
    finally { delete state.pending[id] }
  }
  function startTarget(id) { return startConfiguredTarget(id, cleanConfig(getConfig(id))) }
  async function startVisible() {
    const requests = visible().map(id => ({ id, config: cleanConfig(getConfig(id)) }))
    return Promise.all(requests.map(({ id, config }) => startConfiguredTarget(id, config)))
  }
  function captureVisibleStops() { return visible().flatMap(id => { const run = getRun(id); return run ? [{ targetId: id, runId: run.run_id }] : [] }) }
  async function stopCaptured(snapshot) {
    return Promise.all(snapshot.map(async ({ targetId, runId }) => {
      if (!visible().includes(targetId)) return { targetId, status: 'skipped', reason: '目标已隐藏或移除' }
      if (getRun(targetId)?.run_id !== runId) return { targetId, status: 'skipped', reason: '确认时的运行已结束或更换' }
      try { await api.cancelRun(runId); beginCancel(runId); return { targetId, runId, status: 'stopping' } }
      catch (e) { return { targetId, runId, status: 'error', reason: e.message || String(e) } }
    }))
  }
  return { state, get visibleTargetIds() { return visible() }, getConfig, updateConfig, addTarget, removeTarget, selectTarget, setGridSize, getRun, refreshRunStates, startPolling, stopPolling, startTarget, startVisible, captureVisibleStops, stopCaptured }
}
