// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, KeepAlive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import { scriptsData, store } from './store'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'

const { confirm } = vi.hoisted(() => ({ confirm: Object.assign(vi.fn(async () => false), { cancel: vi.fn() }) }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => confirm }))
let wrapper
const script = { id: 'auto/main.yaml', package: 'auto', name: 'main.yaml', version: 's1', content: 'run:\n  - log: script\n' }
const library = { id: 'auto/_function.yaml', pkg: 'auto', file: '_function.yaml', version: 'f1', functions: ['first', 'chosen'], content: 'functions:\n  first:\n    run: []\n  chosen:\n    run: []\n' }
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks(); confirm.mockClear(); scriptsData.value = []; store.running = false; store.runId = null })
async function setup({ preset = '', delayList = false } = {}) {
  const getScript = vi.spyOn(api, 'getScript').mockResolvedValue(script)
  const getFunction = vi.spyOn(api, 'getFunction').mockResolvedValue(library)
  vi.spyOn(api, 'listScripts').mockImplementation(() => delayList ? new Promise(() => {}) : Promise.resolve([script]))
  vi.spyOn(api, 'listFunctions').mockResolvedValue([library])
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: [] })
  vi.spyOn(api, 'listRunHistory').mockResolvedValue([])
  const panel = ref('script'); let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: ref('auto'), toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: x => x, loadData: vi.fn() })
    if (preset) runner.scriptPanel.selScript.value = preset
    return () => h(KeepAlive, null, { default: () => h(ScriptRunner, {
      key: panel.value, context: panel.value === 'script' ? runner.scriptPanel : runner.functionsPanel,
    }) })
  } }))
  await runner.fnLib.refresh('auto'); await flushPromises()
  return { panel, runner, getScript, getFunction }
}

it('opens a preselected script even when its resource list has not arrived', async () => {
  const { getScript } = await setup({ preset: script.id, delayList: true })
  expect(getScript).toHaveBeenCalledWith(script.id)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  expect(wrapper.text()).not.toContain('打开编辑器')
  expect(wrapper.text()).not.toContain('选择资源开始编辑')
})

it('automatically restores each selected resource on cached panel activation without duplicate loads', async () => {
  const { panel, runner, getScript, getFunction } = await setup()
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  expect(getScript).toHaveBeenCalledTimes(1)
  panel.value = 'function'; await flushPromises()
  await wrapper.find('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  expect(wrapper.find('input[aria-label="函数名称"]').element.value).toBe('chosen')
  panel.value = 'script'; await flushPromises()
  expect(runner.scriptShell.resourceId).toBe(script.id)
  expect(wrapper.find('input[aria-label="脚本名称"]').exists()).toBe(true)
  expect(getScript).toHaveBeenCalledTimes(2)
  // Updating a hidden function list must not steal the shared editor.
  const count = getFunction.mock.calls.length
  await runner.fnLib.refresh('auto'); await flushPromises()
  expect(getFunction).toHaveBeenCalledTimes(count)
  expect(runner.scriptShell.resourceId).toBe(script.id)
  panel.value = 'function'; await flushPromises()
  expect(runner.scriptShell.resourceId).toBe(library.id)
  expect(wrapper.find('input[aria-label="函数名称"]').element.value).toBe('chosen')
  expect(getFunction).toHaveBeenCalledTimes(count + 1)
})

it('does not discard another panel draft when automatic restoration is declined', async () => {
  const { panel, runner, getFunction } = await setup()
  panel.value = 'function'; await flushPromises()
  panel.value = 'script'; await flushPromises()
  const loaded = getFunction.mock.calls.length
  runner.scriptShell.scriptDisplayName = 'unsaved'
  const model = runner.scriptShell.model
  panel.value = 'function'; await flushPromises()
  expect(confirm).toHaveBeenCalled()
  expect(getFunction).toHaveBeenCalledTimes(loaded)
  expect(runner.scriptShell.model).toBe(model)
  expect(runner.scriptShell.dirty).toBe(true)
  expect(wrapper.text()).toContain('保留了当前未保存的修改')
  panel.value = 'script'; await flushPromises()
  expect(wrapper.find('input[aria-label="脚本名称"]').element.value).toBe('unsaved')
})

it('shows a retry state after a failed restoration without repeatedly requesting the resource', async () => {
  const { panel, getScript } = await setup()
  panel.value = 'function'; await flushPromises()
  getScript.mockRejectedValueOnce(new Error('offline'))
  panel.value = 'script'; await flushPromises()
  expect(getScript).toHaveBeenCalledTimes(2)
  expect(wrapper.find('[role="alert"]').text()).toContain('资源未能加载')
  await flushPromises()
  expect(getScript).toHaveBeenCalledTimes(2)
  await wrapper.findAll('button').find(b => b.text() === '重新加载').trigger('click'); await flushPromises()
  expect(getScript).toHaveBeenCalledTimes(3)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
})
