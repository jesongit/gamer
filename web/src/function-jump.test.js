// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, provide, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import { scriptsData, store } from './store'
import { WORKSPACE_CONTEXT_KEY } from './workspace/context'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'
import catalog from '../../tools/yaml-tests/native-functions.json'
import { createCall, createControl } from '../../plugins/gamer-yaml/ui/src/script-editor/factories'

let wrapper
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks(); scriptsData.value = []; store.running = false })

async function setup() {
  const script = { id: 'qa/main.yaml', package: 'qa', name: 'main.yaml', version: 's1', content: 'run:\n  - helper: {}\n  - log: done\n' }
  const file = { id: 'qa/_function_more.yaml', pkg: 'qa', file: '_function_more.yaml', version: 'f1', functions: ['helper', 'local'], content: 'functions:\n  helper:\n    run:\n      - local: {}\n  local:\n    run:\n      - log: local\n' }
  const order = []
  vi.spyOn(api, 'listScripts').mockImplementation(async () => [{ ...script }])
  vi.spyOn(api, 'getScript').mockImplementation(async () => ({ ...script }))
  vi.spyOn(api, 'listFunctions').mockImplementation(async () => [{ ...file }])
  const getFunction = vi.spyOn(api, 'getFunction').mockImplementation(async () => { order.push('load-function'); return { ...file } })
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: catalog })
  const saveScript = vi.spyOn(api, 'updateScript').mockImplementation(async (id, data) => { order.push('save-script'); Object.assign(script, data, { version: 's2' }); return { ...script } })
  const saveFunction = vi.spyOn(api, 'updateFunction').mockImplementation(async (id, data) => { order.push('save-function'); Object.assign(file, data, { version: 'f2' }); return { ...file } })
  let runner
  const panel = ref('gamer-yaml:automation')
  const openPanel = vi.fn(async key => { panel.value = key })
  wrapper = mount(defineComponent({ setup() {
    provide(WORKSPACE_CONTEXT_KEY, { uiBridge: { workspace: { openPanel } } })
    runner = useConsoleScriptRunner({ packageId: ref('qa'), toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: x => x, loadData: vi.fn() })
    return () => h(ScriptRunner, { key: panel.value, context: panel.value.endsWith('functions') ? runner.functionsPanel : runner.scriptPanel })
  } }))
  await runner.fnLib.refresh('qa')
  await flushPromises()
  return { runner, script, file, order, openPanel, saveScript, saveFunction, getFunction }
}

it('跳转位于运行前，仅自定义函数显示，保存后跨文件/同文件打开并返回来处', async () => {
  const { runner, script, order, openPanel, saveFunction } = await setup()
  expect(wrapper.findAll('.jump-function')).toHaveLength(1)
  expect(wrapper.get('.jump-function').element.nextElementSibling.classList.contains('test-from')).toBe(true)
  const sourceStep = runner.scriptShell.model.run[0]
  runner.scriptShell.stack.apply({ type: 'update_step', path: ['run', 1], fields: { args: { kind: 'value', cell: { lit: 'edited' } } } })
  await wrapper.get('.jump-function').trigger('click')
  await flushPromises()
  expect(order.indexOf('save-script')).toBeLessThan(order.indexOf('load-function'))
  expect(script.content).toContain('edited')
  expect(openPanel).toHaveBeenLastCalledWith('gamer-yaml:functions')
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('helper')
  expect(runner.scriptShell.jumpStack).toHaveLength(1)
  await wrapper.get('.jump-function').trigger('click')
  await flushPromises()
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('local')
  expect(runner.scriptShell.jumpStack).toHaveLength(2)
  expect(wrapper.find('.jump-function').exists()).toBe(false)
  runner.scriptShell.stack.apply({ type: 'update_step', path: ['functions', 'local', 'run', 0], fields: { args: { kind: 'value', cell: { lit: 'saved-before-return' } } } })
  await wrapper.get('.jump-back').trigger('click')
  await flushPromises()
  expect(saveFunction).toHaveBeenLastCalledWith('qa/_function_more.yaml', expect.objectContaining({ content: expect.stringContaining('saved-before-return') }))
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('helper')
  await wrapper.get('.jump-back').trigger('click')
  await flushPromises()
  expect(openPanel).toHaveBeenLastCalledWith('gamer-yaml:automation')
  expect(runner.scriptShell.resourceId).toBe('qa/main.yaml')
  expect(runner.scriptShell.selectedUuid).toBe(runner.scriptShell.model.run[0].uuid)
  expect(runner.scriptShell.selectedUuid).not.toBe(sourceStep.uuid)
})

it('保存冲突时不加载目标、不跳转，目标读取失败也保留当前模型和历史', async () => {
  const { runner, saveScript, getFunction, openPanel } = await setup()
  const model = runner.scriptShell.model
  saveScript.mockRejectedValueOnce(Object.assign(new Error('版本冲突'), { status: 409, data: { code: 'version_conflict' } }))
  await wrapper.get('.jump-function').trigger('click')
  await flushPromises()
  expect(openPanel).not.toHaveBeenCalled()
  expect(getFunction).not.toHaveBeenCalled()
  expect(runner.scriptShell.model).toBe(model)
  expect(runner.scriptShell.conflict).toBeTruthy()
  runner.scriptShell.dismissConflict()
  getFunction.mockRejectedValueOnce(new Error('离线'))
  await wrapper.get('.jump-function').trigger('click')
  await flushPromises()
  expect(openPanel).not.toHaveBeenCalled()
  expect(runner.scriptShell.model).toBe(model)
  expect(runner.scriptShell.jumpStack).toHaveLength(0)
})

it('嵌套步骤也可跳转，保存完成前禁用重复点击且不提前切换', async () => {
  const { runner, saveScript, openPanel } = await setup()
  const loop = createControl('repeat')
  loop.body.push(createCall('helper'))
  runner.scriptShell.stack.apply({ type: 'insert_step', path: ['run'], index: 0, step: loop })
  await flushPromises()
  await wrapper.get('.kind-repeat .expand-btn').trigger('click')
  const button = wrapper.get('.kind-repeat .branch-container .jump-function')
  let finish
  const original = saveScript.getMockImplementation()
  saveScript.mockImplementationOnce((...args) => new Promise(resolve => { finish = async () => resolve(await original(...args)) }))
  await button.trigger('click')
  await flushPromises()
  expect(button.element.disabled).toBe(true)
  expect(openPanel).not.toHaveBeenCalled()
  await button.trigger('click')
  expect(saveScript).toHaveBeenCalledTimes(1)
  await finish()
  await flushPromises()
  expect(openPanel).toHaveBeenCalledTimes(1)
  expect(runner.functionsPanel.editFocusFn.value).toBe('helper')
})
