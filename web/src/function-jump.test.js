// @vitest-environment happy-dom
// v2 source-first workspace replaces form-card jumps. Whole-library navigation
// must preserve source, protect conflicts, and retain an explicit function run target.
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import { scriptsData, store } from './store'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import AutomationWorkbench from '../../plugins/gamer-yaml/ui/src/components/console/AutomationWorkbench.vue'
const { confirm } = vi.hoisted(() => ({ confirm: Object.assign(vi.fn(async () => false), { cancel: vi.fn() }) }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => confirm }))
let wrapper
const button = label => wrapper.findAll('button').find(item => item.text() === (label === '保存' ? '保存草稿' : label))
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks(); confirm.mockReset().mockResolvedValue(false); scriptsData.value = []; store.running = false; store.deviceId = '' })
async function setup() {
  const script = { id: 'qa/main.yaml', package: 'qa', name: 'main.yaml', version: 's1', content: '# retained script\nrun:\n  - helper: {}\n  - finish: {template: done.png}\n' }
  const file = { id: 'qa/_function_more.yaml', pkg: 'qa', file: '_function_more.yaml', version: 'f1', functions: ['helper', 'local'], content: '# retained library\nfunctions:\n  helper:\n    run:\n      - local: {}\n  local:\n    run:\n      - log: local\n' }
  vi.spyOn(api, 'listScripts').mockResolvedValue([script]); vi.spyOn(api, 'getScript').mockResolvedValue(script)
  vi.spyOn(api, 'listFunctions').mockResolvedValue([file]); vi.spyOn(api, 'listTemplates').mockResolvedValue([])
  const getFunction = vi.spyOn(api, 'getFunction').mockResolvedValue(file)
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: [] }); vi.spyOn(api, 'listExtensions').mockResolvedValue([])
  vi.spyOn(api, 'listRunHistory').mockResolvedValue([])
  const saveScript = vi.fn().mockResolvedValue({})
  vi.spyOn(api,'callExtension').mockImplementation(async (_id,action,values)=>{
    if(action==='authoring.get')return{script:{head_version:'1.0'}}
    if(action==='authoring.save'){await saveScript(values);return{script:{head_version:'1.1',versions:[{version:'1.1',diagnostics:[]}]}}}
    return{}
  })
  const saveFunction = vi.spyOn(api, 'updateFunction').mockResolvedValue({ ...file, version: 'f2' })
  let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: ref('qa'), toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: x => x, loadData: vi.fn() })
    return () => h(AutomationWorkbench, { context: { scripts: runner.scriptPanel, functions: runner.functionsPanel, templates: {} } })
  } }), { global: { stubs: { TemplateCapture: true } } })
  await flushPromises()
  return { runner, script, file, saveScript, saveFunction, getFunction }
}
it('函数在脚本内新增，保存保留整个脚本及注释', async () => {
  const { script, saveScript } = await setup()
  expect(button('函数')).toBeUndefined()
  await wrapper.get('[aria-label="新函数名"]').setValue('local')
  await button('新增函数').trigger('click'); await flushPromises()
  const source = wrapper.get('textarea').element.value
  expect(source.startsWith(script.content)).toBe(true)
  expect(source).toContain('functions:\n  local:')
  await button('保存').trigger('click'); await flushPromises()
  expect(saveScript.mock.calls[0][0].yaml).toBe(source)
  expect(wrapper.get('textarea').element.value).toBe(source)
})
it('保存冲突不覆盖服务端，取消导航保留当前源码', async () => {
  const { saveScript, getFunction } = await setup()
  const source = '# edited\nrun: []\n'; await wrapper.get('textarea').setValue(source)
  saveScript.mockRejectedValueOnce(Object.assign(new Error('版本冲突'), { status: 409 }))
  await button('保存').trigger('click'); await flushPromises()
  expect(wrapper.get('[role="alert"]').text()).toContain('版本冲突')
  await button('模板').trigger('click'); await flushPromises()
  expect(getFunction).not.toHaveBeenCalled(); expect(wrapper.get('.tab-btn.active').text()).toBe('脚本库')
  expect(wrapper.get('textarea').element.value).toBe(source)
  expect(saveScript).toHaveBeenCalledTimes(1)
})
it('保存完成前禁止重复提交和跨面板导航，成功后才能切换页面', async () => {
  const { saveScript, getFunction } = await setup()
  let finish; saveScript.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
  await wrapper.get('textarea').setValue('# change\nrun: []\n')
  await button('保存').trigger('click'); await flushPromises()
  expect(button('保存').element.disabled).toBe(true)
  await button('保存').trigger('click'); await button('模板').trigger('click'); await flushPromises()
  expect(saveScript).toHaveBeenCalledTimes(1); expect(getFunction).not.toHaveBeenCalled()
  finish({ version: 's2' }); await flushPromises()
  await button('模板').trigger('click'); await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('模板'); expect(getFunction).not.toHaveBeenCalled()
})
