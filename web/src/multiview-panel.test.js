// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { api } from './api'
import { store, runRegistry, projectDeviceRun, applyRunRecord, devicesData } from './store'
import { packageStore, currentPackageId, selectPackage } from './package-store'
import { multiviewWorkspace as workspace } from './console/multiview-workspace'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
let wrapper, runner
beforeEach(async () => {
  for (const [name,result] of Object.entries({ listScripts:[],listFunctions:[],listTemplates:[],getRunnerFunctions:{functions:[]},listExtensions:[] })) vi.spyOn(api,name).mockResolvedValue(result)
  vi.spyOn(api,'getRun').mockImplementation(async id=>({...runRegistry.byId[id],run_id:id,state:'success'}))
  packageStore.packages=[{id:'one'},{id:'two'}]; packageStore.currentPackageId='one'
  workspace.state.targetIds=[]; workspace.state.configs=Object.create(null); workspace.state.selectedTargetId=null
  workspace.addTarget('a'); workspace.addTarget('b')
  workspace.updateConfig('a',{packageId:'one',scriptId:'one/a.yaml',runnerId:'gamer-yaml',args:{account:'A'}})
  workspace.updateConfig('b',{packageId:'two',scriptId:'two/b.yaml',runnerId:'gamer-yaml',args:{account:'B'}})
  store.deviceId='a'
  wrapper=mount(defineComponent({setup(){runner=useConsoleScriptRunner({multiviewWorkspace:workspace,packageId:currentPackageId,restorePackage:selectPackage,toast:vi.fn(),consoleRuntime:{startLogPolling:vi.fn(),stopLogPolling:vi.fn()},templateNames:ref([]),tplShortName:x=>x,loadData:vi.fn()});return()=>h('div')}}))
  runner.projectTargetConfig('a'); await flushPromises()
})
afterEach(()=>{wrapper?.unmount();vi.restoreAllMocks();store.deviceId=null;projectDeviceRun(null);runRegistry.byId=Object.create(null);runRegistry.activeByDevice=Object.create(null);workspace.stopPolling();devicesData.value=[]})
it('source drafts reject even same-package target changes without moving focus or editor',async()=>{
  workspace.updateConfig('b',{packageId:'one',scriptId:'one/b.yaml'})
  runner.scriptPanel.sourceEditorDirty.value=true
  expect(runner.beforeTargetChange('b')).toBe(false)
  expect(store.deviceId).toBe('a');expect(currentPackageId.value).toBe('one');expect(runner.scriptPanel.selScript.value).toBe('one/a.yaml')
  runner.scriptPanel.sourceEditorDirty.value=false
  expect(runner.beforeTargetChange('b')).toBe(true)
})
it('successful target projection preserves each package/script/args and never writes into previous target',async()=>{
  expect(runner.projectTargetConfig('b')).toBe(true);store.deviceId='b';await nextTick()
  expect(currentPackageId.value).toBe('two');expect(runner.scriptPanel.selScript.value).toBe('two/b.yaml')
  expect(workspace.getConfig('a')).toMatchObject({packageId:'one',scriptId:'one/a.yaml',args:{account:'A'}})
  expect(workspace.getConfig('b').args).toEqual({account:'B'})
  expect(runner.projectTargetConfig('a')).toBe(true);store.deviceId='a';await nextTick()
  expect(workspace.getConfig('b').scriptId).toBe('two/b.yaml')
})
it('unconfigured or missing-package target stays empty and can be repaired',async()=>{
  workspace.addTarget('empty');expect(runner.projectTargetConfig('empty')).toBe(true);store.deviceId='empty';await nextTick()
  expect(currentPackageId.value).toBe(null);expect(runner.scriptPanel.selScript.value).toBe('')
  workspace.updateConfig('b',{packageId:'missing'});expect(runner.projectTargetConfig('b')).toBe(true)
  expect(store.deviceId).toBe('empty');expect(currentPackageId.value).toBe(null)
})
it('focused run projection never stops background instances',()=>{
  applyRunRecord({run_id:'a-run',device_id:'a',state:'running',entrypoint:'one/a.yaml'})
  applyRunRecord({run_id:'b-run',device_id:'b',state:'stopping',entrypoint:'two/b.yaml'})
  store.deviceId='b';projectDeviceRun('b');expect(store.runId).toBe('b-run');expect(store.runStep).toContain('停止')
  store.deviceId='idle';projectDeviceRun('idle');expect(store.running).toBe(false)
  expect(runRegistry.activeByDevice.a).toBe('a-run');expect(runRegistry.activeByDevice.b).toBe('b-run')
})
it('configure-only saves isolated per-target overrides with no run, then bulk payloads use those exact overrides',async()=>{
  const run=vi.spyOn(api,'run').mockImplementation(async req=>({run_id:`run-${req.device_id}`,state:'running'}))
  vi.spyOn(api,'getEntrypointParams').mockResolvedValue({schema:[{name:'account',type:'string',required:true},{name:'count',type:'integer',required:false,default:5},{name:'enabled',type:'boolean',required:false,default:true}]})
  await runner.scriptPanel.configureTargetScript()
  expect(runner.runArgsFlow.modal.submitLabel).toBe('保存配置');expect(runner.runArgsFlow.modal.params).toHaveLength(3)
  expect(run).not.toHaveBeenCalled()
  const invalid=await runner.runArgsFlow.confirm({count:'bad'})
  expect(invalid.ok).toBe(false);expect(runner.runArgsFlow.modal.open).toBe(true);expect(run).not.toHaveBeenCalled()
  expect((await runner.runArgsFlow.confirm({account:'A',count:0,enabled:false})).ok).toBe(true)
  runner.projectTargetConfig('b');store.deviceId='b';await nextTick()
  await runner.scriptPanel.configureTargetScript();await runner.runArgsFlow.confirm({account:'B',count:2,enabled:true})
  expect(run).not.toHaveBeenCalled()
  expect(workspace.getConfig('a').args).toEqual({account:'A',count:0,enabled:false})
  expect(workspace.getConfig('b').args).toEqual({account:'B',count:2,enabled:true})
  const {devicesData}=await import('./store');devicesData.value=[{id:'a',status:'online'},{id:'b',status:'online'}]
  await workspace.startVisible()
  expect(run.mock.calls.map(([req])=>({id:req.device_id,args:req.payload.args}))).toEqual([{id:'a',args:{account:'A',count:0,enabled:false}},{id:'b',args:{account:'B',count:2,enabled:true}}])
})
it('configure-only keeps parameterless scripts in explicit save flow and cancels stale target/schema confirmations',async()=>{
  const run=vi.spyOn(api,'run')
  const params=vi.spyOn(api,'getEntrypointParams').mockResolvedValue({schema:[]})
  await runner.scriptPanel.configureTargetScript();expect(runner.runArgsFlow.modal.open).toBe(true);expect(run).not.toHaveBeenCalled()
  await runner.runArgsFlow.confirm({});expect(workspace.getConfig('a').args).toEqual({});expect(run).not.toHaveBeenCalled()
  let resolve;params.mockReturnValueOnce(new Promise(done=>{resolve=done}))
  const pending=runner.scriptPanel.configureTargetScript()
  runner.projectTargetConfig('b');store.deviceId='b';await nextTick();resolve({schema:[]});await pending
  expect(runner.runArgsFlow.modal.open).toBe(false)
  expect((await runner.runArgsFlow.confirm({account:'wrong'})).ok).toBe(false)
  expect(workspace.getConfig('b').args).toEqual({account:'B'});expect(run).not.toHaveBeenCalled()
})
it('right-panel run preserves configured optional zero/false overrides without mixing another target cache',async()=>{
  const {scriptsData}=await import('./store');scriptsData.value=[{id:'one/a.yaml',name:'A'}]
  const run=vi.spyOn(api,'run').mockResolvedValue({run_id:'configured',state:'running'})
  vi.spyOn(api,'deviceRun').mockResolvedValue({active:false})
  vi.spyOn(api,'getEntrypointParams').mockResolvedValue({schema:[{name:'count',type:'integer',required:false,default:5},{name:'enabled',type:'boolean',required:false,default:true}]})
  workspace.updateConfig('a',{args:{count:0,enabled:false}})
  await runner.scriptPanel.runScript()
  expect(run).toHaveBeenCalledWith(expect.objectContaining({device_id:'a',payload:{args:{count:0,enabled:false}}}))
  expect(workspace.getConfig('a').args).toEqual({count:0,enabled:false})
})
it('same-target script changes invalidate both late configuration schemas and already-open save forms',async()=>{
  const run=vi.spyOn(api,'run');let resolve
  const params=vi.spyOn(api,'getEntrypointParams').mockReturnValueOnce(new Promise(done=>{resolve=done}))
  const pending=runner.scriptPanel.configureTargetScript()
  runner.scriptPanel.selScript.value='one/changed.yaml';await nextTick()
  resolve({schema:[{name:'account',type:'string',required:true}]});await pending
  expect(runner.runArgsFlow.modal.open).toBe(false)
  params.mockResolvedValue({schema:[]});await runner.scriptPanel.configureTargetScript()
  expect(runner.runArgsFlow.modal.open).toBe(true)
  runner.scriptPanel.selScript.value='one/latest.yaml';await nextTick()
  expect((await runner.runArgsFlow.confirm({account:'stale'})).ok).toBe(false)
  expect(workspace.getConfig('a')).toMatchObject({scriptId:'one/latest.yaml',args:{}})
  expect(run).not.toHaveBeenCalled()
})
