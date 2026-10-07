import { test } from 'vitest'
import assert from 'node:assert/strict'
import { createMultiviewWorkspace, MULTIVIEW_STORAGE_KEY } from './console/multiview-state.mjs'
function setup(overrides = {}, saved) {
  const registry = { byId: {}, activeByDevice: {} }, calls = [], disk = {}
  if (saved) disk[MULTIVIEW_STORAGE_KEY] = JSON.stringify(saved)
  const applyRunRecord = rec => { registry.byId[rec.run_id] = rec; if (['success','cancelled','failed'].includes(rec.state)) { if (registry.activeByDevice[rec.device_id] === rec.run_id) delete registry.activeByDevice[rec.device_id] } else registry.activeByDevice[rec.device_id] = rec.run_id }
  const api = { deviceRun: async () => ({ active: false }), getRun: async id => ({ ...registry.byId[id], state: 'success' }), run: async request => { calls.push(request); return { run_id: 'run-' + request.device_id, state: 'running' } }, cancelRun: async id => calls.push(id), ...overrides }
  const workspace = createMultiviewWorkspace({ storage: { getItem: key => disk[key], setItem: (key,value) => { disk[key] = value } }, registry, applyRunRecord, beginCancel: id => { registry.byId[id].state = 'stopping' }, api })
  return { workspace, registry, calls, applyRunRecord, disk }
}
const configure = (w, id) => { w.addTarget(id); w.updateConfig(id, { packageId: 'pkg', scriptId: 'pkg/test.yaml', runnerId: 'yaml', args: { n: id } }) }
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
test('layout deduplicates and restores without auto-starting; 9 to 4 retains hidden targets', async () => {
  const { workspace: w, calls, disk } = setup({}, { targetIds: ['a','a','b'], gridSize: 9 })
  assert.deepEqual(w.state.targetIds, ['a','b']); assert.equal(calls.length, 0)
  for (const id of ['c','d','e']) configure(w,id)
  await w.selectTarget('e'); await w.setGridSize(4)
  assert.deepEqual(w.visibleTargetIds, ['a','b','c','d']); assert.equal(w.state.targetIds[4], 'e'); assert.equal(w.state.selectedTargetId, 'a')
  assert.equal(JSON.parse(disk[MULTIVIEW_STORAGE_KEY]).configs.e.args.n, 'e')
})
test('guard refusal and concurrent selection never project or change focus', async () => {
  const { workspace: w } = setup(); configure(w,'a'); configure(w,'b'); await w.selectTarget('a')
  let projected = false
  assert.equal(await w.selectTarget('b',{ beforeChange: () => false, project: () => { projected = true } }), false)
  assert.equal(projected,false); assert.equal(w.state.selectedTargetId,'a')
  const gate = deferred(); const pending = w.selectTarget('b',{ beforeChange: () => gate.promise })
  assert.equal(await w.selectTarget(null),false); gate.resolve(true); assert.equal(await pending,true)
})
test('batch start captures visible targets/config and late replies stay bound to request', async () => {
  const gate = deferred(); const { workspace: w, calls, registry } = setup({ run: async req => { calls.push(req); await gate.promise; return { run_id: req.device_id, state:'running' } } })
  for (const id of ['a','b','c','d','hidden']) configure(w,id)
  const running = w.startVisible(); w.updateConfig('a',{ scriptId:'pkg/changed.yaml' }); await w.selectTarget('b'); await w.removeTarget('c')
  gate.resolve(); const results = await running
  assert.equal(results.length,4); assert.equal(calls[0].entrypoint,'pkg/test.yaml'); assert.equal(calls.some(req => req.device_id === 'hidden'),false)
  assert.equal(registry.byId.a.device_id,'a'); assert.equal(registry.byId.c.device_id,'c')
})
test('stop snapshot never cancels replacement runs or hidden targets', async () => {
  const { workspace:w, applyRunRecord, calls } = setup()
  for (const id of ['a','b','c','d','hidden']) { configure(w,id); applyRunRecord({ run_id:id, device_id:id, state:'running' }) }
  const snapshot = w.captureVisibleStops(); applyRunRecord({ run_id:'replacement', device_id:'a',state:'running' }); await w.removeTarget('b')
  const results = await w.stopCaptured(snapshot)
  assert.equal(results[0].status,'skipped'); assert.deepEqual(calls,['c','d'])
})
test('refresh polls hidden and removed runs; failed target does not mask other results', async () => {
  const seen=[]; const { workspace:w, applyRunRecord, registry } = setup({ deviceRun: async id => { seen.push(id); if (id==='b') throw new Error('offline'); return { active:false } } })
  for (const id of ['a','b','c','d','hidden']) { configure(w,id); applyRunRecord({ run_id:id, device_id:id,state:'running' }) }
  await w.removeTarget('a'); await w.refreshRunStates()
  assert.deepEqual(new Set(seen),new Set(['a','b','c','d','hidden'])); assert.equal(w.state.errors.b,'offline'); assert.equal(registry.activeByDevice.b,'b'); assert.equal(registry.byId.hidden.state,'success')
})
test('configuration incomplete and already-running targets skip individually', async () => {
  const { workspace:w, applyRunRecord }=setup(); configure(w,'a'); w.addTarget('b'); applyRunRecord({ run_id:'active',device_id:'a',state:'running' })
  const results=await w.startVisible(); assert.deepEqual(results.map(row=>row.status),['skipped','skipped'])
})
test('restored configuration rejects malformed types and safely keeps special target IDs', () => {
  const { workspace:w }=setup({}, { targetIds:['__proto__','constructor','normal'], configs: JSON.parse('{"__proto__":{"packageId":5,"scriptId":[],"runnerId":{},"args":[]},"constructor":{"packageId":"p","scriptId":"p/s","runnerId":"r","args":{"x":1}},"normal":false}') })
  assert.deepEqual(w.getConfig('__proto__'),{ packageId:'',scriptId:'',runnerId:'',args:{} })
  assert.equal(w.getConfig('constructor').args.x,1)
  assert.equal(Object.getPrototypeOf(w.state.configs),null)
})
test('batch freezes every configuration before the first request can trigger another edit', async () => {
  const { workspace:w, calls } = setup({ run:async req => { calls.push(req); w.updateConfig('b',{ args:{ changed:true } }); return { run_id:req.device_id,state:'running' } } })
  configure(w,'a'); configure(w,'b'); await w.startVisible()
  assert.deepEqual(calls[1].payload.args,{ n:'b' })
})
test('selection lock also protects target removal and grid reduction', async () => {
  const {workspace:w}=setup(); for (const id of ['a','b','c','d','e']) configure(w,id)
  await w.setGridSize(9); const gate=deferred(); const selection=w.selectTarget('e',{beforeChange:()=>gate.promise})
  assert.equal(await w.removeTarget('e'),false); assert.equal(await w.setGridSize(4),false)
  gate.resolve(true); await selection; assert.equal(w.state.selectedTargetId,'e')
})
test('late recovery cannot overwrite a newly registered run', async () => {
  const gate=deferred(); const {workspace:w,registry,applyRunRecord}=setup({deviceRun:()=>gate.promise})
  configure(w,'a'); const recovery=w.refreshRunStates(); applyRunRecord({run_id:'new',device_id:'a',state:'running'})
  gate.resolve({active:true,run:{run_id:'old',device_id:'a',state:'running'}}); await recovery
  assert.equal(registry.activeByDevice.a,'new')
})

test('removed target configuration survives refresh and explicit re-add without restoring a tile or running', async () => {
  const first = setup(); configure(first.workspace, 'remembered'); configure(first.workspace, 'visible')
  const config = { ...first.workspace.getConfig('remembered'), args: { account: 'A', loops: 3 } }
  first.workspace.updateConfig('remembered', config)
  await first.workspace.removeTarget('remembered')
  const restored = setup({}, JSON.parse(first.disk[MULTIVIEW_STORAGE_KEY]))
  assert.deepEqual(restored.workspace.state.targetIds, ['visible'])
  assert.deepEqual(restored.workspace.getConfig('remembered'), config)
  assert.equal(restored.calls.length, 0)
  assert.equal(restored.workspace.addTarget('remembered'), true)
  assert.deepEqual(restored.workspace.getConfig('remembered'), config)
  assert.equal(restored.calls.length, 0)
})
