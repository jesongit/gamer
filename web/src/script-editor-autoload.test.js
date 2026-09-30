// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, KeepAlive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import { scriptsData, store, runRegistry } from './store'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'
import AutomationWorkbench from '../../plugins/gamer-yaml/ui/src/components/console/AutomationWorkbench.vue'
import { load } from 'js-yaml'
import { requestAutomationEditor } from '../../plugins/gamer-yaml/ui/src/components/console/automationEditorBridge'

const { confirm } = vi.hoisted(() => ({ confirm: Object.assign(vi.fn(async () => false), { cancel: vi.fn() }) }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => confirm }))
let wrapper
const script = { id: 'auto/main.yaml', package: 'auto', name: 'main.yaml', version: 's1', content: 'run:\n  - log: script\n' }
const library = { id: 'auto/_function.yaml', pkg: 'auto', file: '_function.yaml', version: 'f1', functions: ['first', 'chosen'], content: 'functions:\n  first:\n    run: []\n  chosen:\n    run: []\n' }
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks(); confirm.mockReset().mockResolvedValue(false); scriptsData.value = []; store.running = false; store.runId = null; store.deviceId = ''; runRegistry.byId = {}; runRegistry.activeByDevice = {}; runRegistry.last = null })
async function setup({ preset = '', delayList = false, workbench = false } = {}) {
  const getScript = vi.spyOn(api, 'getScript').mockResolvedValue(script)
  const getFunction = vi.spyOn(api, 'getFunction').mockResolvedValue(library)
  vi.spyOn(api, 'listScripts').mockImplementation(() => delayList ? new Promise(() => {}) : Promise.resolve([script]))
  vi.spyOn(api, 'listFunctions').mockResolvedValue([library])
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: [] })
  vi.spyOn(api, 'listRunHistory').mockResolvedValue([])
  vi.spyOn(api, 'getRunEvents').mockResolvedValue({ events: [], has_more: false })
  const panel = ref('script'); let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: ref('auto'), toast: vi.fn(), consoleRuntime: { startLogPolling: vi.fn() }, templateNames: ref([]), tplShortName: x => x, loadData: vi.fn() })
    if (preset) runner.scriptPanel.selScript.value = preset
    return () => workbench ? h(AutomationWorkbench, { context: { scripts: runner.scriptPanel, functions: runner.functionsPanel, templates: {
      packageId: 'auto', templates: [], tplSearch: '', stageReady: false,
    } } }) : h(KeepAlive, null, { default: () => h(ScriptRunner, {
      key: panel.value, context: panel.value === 'script' ? runner.scriptPanel : runner.functionsPanel,
    }) })
  } }))
  await runner.fnLib.refresh('auto'); await flushPromises()
  return { panel, runner, getScript, getFunction }
}

it('自动化子页签切换保留选择，取消切换保留当前页签和未保存内容', async () => {
  const { runner } = await setup({ workbench: true })
  const tab = name => wrapper.findAll('.tab-btn').find(b => b.text() === name)
  expect(wrapper.get('.tab-btn.active').text()).toBe('脚本')
  runner.scriptShell.scriptDisplayName = '未保存脚本'
  await tab('函数').trigger('click'); await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('脚本')
  expect(runner.scriptShell.scriptDisplayName).toBe('未保存脚本')
  confirm.mockResolvedValueOnce(true)
  await tab('函数').trigger('click'); await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('函数')
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  await tab('脚本').trigger('click'); await flushPromises()
  expect(runner.scriptShell.resourceId).toBe(script.id)
  await tab('函数').trigger('click'); await flushPromises()
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('chosen')
  await tab('模板').trigger('click'); await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('模板')
  expect(wrapper.find('.tpl-list').exists()).toBe(true)
  expect(wrapper.find('.se-canvas').exists()).toBe(false)
  await tab('函数').trigger('click'); await flushPromises()
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('chosen')
})

it('函数原文未保存时取消子页签切换保持原文；视频草稿打开时切回脚本', async () => {
  const { runner } = await setup({ workbench: true })
  const tab = name => wrapper.findAll('.tab-btn').find(b => b.text() === name)
  await tab('函数').trigger('click'); await flushPromises()
  await wrapper.findAll('button').find(b => b.text() === '原文编辑').trigger('click'); await flushPromises()
  const textarea = wrapper.get('.raw-editor')
  await textarea.setValue(textarea.element.value + '\n# draft\n')
  await tab('脚本').trigger('click'); await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('函数')
  expect(wrapper.get('.raw-editor').element.value).toContain('# draft')
  expect(runner.rawEditor.dirty.value).toBe(true)
  runner.functionsPanel.cancelRawScript()
  await flushPromises()
  requestAutomationEditor('auto', script.id)
  await flushPromises()
  expect(wrapper.get('.tab-btn.active').text()).toBe('脚本')
  expect(runner.scriptShell.resourceId).toBe(script.id)
})

it('opens a preselected script even when its resource list has not arrived', async () => {
  const { getScript } = await setup({ preset: script.id, delayList: true })
  expect(getScript).toHaveBeenCalledWith(script.id)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  expect(wrapper.text()).not.toContain('打开编辑器')
  expect(wrapper.text()).not.toContain('选择资源开始编辑')
})

it('清空脚本选择同时清空画布，取消切换时下拉与画布仍对应', async () => {
  const { runner } = await setup()
  runner.scriptShell.scriptDisplayName = 'dirty'
  await wrapper.get('select.sp-name').setValue(''); await flushPromises()
  expect(wrapper.get('select.sp-name').element.value).toBe(script.id)
  expect(runner.scriptShell.resourceId).toBe(script.id)
  confirm.mockResolvedValueOnce(true)
  await wrapper.get('select.sp-name').setValue(''); await flushPromises()
  expect(runner.scriptPanel.selScript.value).toBe('')
  expect(wrapper.find('.se-canvas').exists()).toBe(false)
})

it('取消函数切换后下拉恢复当前函数', async () => {
  const { panel, runner } = await setup()
  panel.value = 'function'; await flushPromises()
  runner.functionsPanel.renameEditingFunction('first', 'draft')
  await flushPromises()
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  expect(wrapper.get('select[aria-label="选择函数"]').element.value).toBe(`${library.id}#draft`)
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('draft')
})

it.each(['script', 'function'])('删除未保存的新 %s 不会删除先前所选资源', async kind => {
  const { panel } = await setup()
  panel.value = kind; await flushPromises()
  const update = vi.spyOn(api, 'updateFunction').mockResolvedValue(library)
  const remove = vi.spyOn(api, 'deleteScript').mockResolvedValue({})
  await wrapper.findAll('button').find(b => b.text() === (kind === 'function' ? '新建函数' : '新建脚本')).trigger('click'); await flushPromises()
  confirm.mockResolvedValueOnce(true)
  await wrapper.findAll('button').find(b => b.text() === '删除').trigger('click'); await flushPromises()
  expect(update).not.toHaveBeenCalled()
  expect(remove).not.toHaveBeenCalled()
})

it('删除未保存改名的函数按磁盘身份定位，不删除其他函数', async () => {
  const { panel, runner } = await setup()
  panel.value = 'function'; await flushPromises()
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  runner.functionsPanel.renameEditingFunction('chosen', 'renamed'); await flushPromises()
  const update = vi.spyOn(api, 'updateFunction').mockResolvedValue(library)
  confirm.mockResolvedValueOnce(true)
  await wrapper.findAll('button').find(b => b.text() === '删除').trigger('click'); await flushPromises()
  expect(Object.keys(load(update.mock.calls[0][1].content).functions)).toEqual(['first'])
})

it('列表中的函数已被移除时不显示库内第一个函数', async () => {
  const { panel, getFunction } = await setup()
  panel.value = 'function'; await flushPromises()
  getFunction.mockResolvedValue({ ...library, content: 'functions:\n  first:\n    run: []\n' })
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  expect(wrapper.find('.se-canvas').exists()).toBe(false)
  expect(wrapper.get('[role="alert"]').text()).toContain('资源未能加载')
})

it('旧脚本加载失败不会清掉后来打开的函数', async () => {
  const { runner, getScript } = await setup()
  let reject
  getScript.mockImplementationOnce(() => new Promise((_, fail) => { reject = fail }))
  const old = runner.scriptPanel.editCurrentTarget()
  await runner.functionsPanel.editFunction({ fileId: library.id, name: 'chosen' })
  reject(new Error('old request failed')); await old
  expect(runner.scriptShell.resourceId).toBe(library.id)
  expect(runner.functionsPanel.editFocusFn.value).toBe('chosen')
})

it('函数定义预览只展示指定函数', async () => {
  const { runner } = await setup()
  await runner.scriptPanel.openScriptTarget({ target: 'chosen' })
  expect(runner.scriptPanel.resourcePreview.model.functions.map(f => f.name)).toEqual(['chosen'])
})

it('外部草稿打开请求不覆盖当前未保存编辑，也不跨配置包打开', async () => {
  const { runner, getScript } = await setup()
  runner.scriptShell.scriptDisplayName = 'keep-draft'
  const model = runner.scriptShell.model
  getScript.mockClear()
  requestAutomationEditor('auto', script.id); await flushPromises()
  expect(getScript).not.toHaveBeenCalled()
  expect(runner.scriptShell.model).toBe(model)
  requestAutomationEditor('another', 'another/main.yaml'); await flushPromises()
  expect(runner.scriptPanel.selScript.value).toBe(script.id)
  expect(getScript).not.toHaveBeenCalled()
})

it('恢复函数运行状态不改写当前脚本选择和编辑模型', async () => {
  const { runner } = await setup()
  store.deviceId = 'restore-device'
  const rec = { run_id: 'restore-selection', device_id: store.deviceId, entrypoint: 'auto#chosen', state: 'running' }
  vi.spyOn(api, 'deviceRun').mockResolvedValue({ active: true, run: rec })
  vi.spyOn(api, 'getRun').mockResolvedValue(rec)
  const model = runner.scriptShell.model
  await runner.restoreRunState(); await flushPromises()
  expect(runner.scriptPanel.selScript.value).toBe(script.id)
  expect(runner.scriptPanel.scriptMode.value).toBe('edit')
  expect(runner.scriptShell.model).toBe(model)
})

it('参数读取期间切换设备不会向新设备提交旧运行', async () => {
  const { runner } = await setup()
  store.deviceId = 'device-before'
  let resolve
  vi.spyOn(api, 'getEntrypointParams').mockImplementation(() => new Promise(done => { resolve = done }))
  const run = vi.spyOn(api, 'run').mockResolvedValue({})
  const pending = runner.scriptPanel.runScript()
  expect(runner.startPending.value).toBe(true)
  store.deviceId = 'device-after'
  resolve({ schema: [] }); await pending
  expect(run).not.toHaveBeenCalled()
  expect(runner.startPending.value).toBe(false)
})

it.each(['保存原文', '运行'])('第二个函数原文改名后点击 %s 仍使用当前函数', async action => {
  const { panel, runner, getFunction } = await setup()
  const run = vi.spyOn(runner.functionsPanel, 'runFunction').mockResolvedValue(undefined)
  store.deviceId = 'raw-device'
  let current = { ...library }
  getFunction.mockImplementation(async () => ({ ...current }))
  vi.spyOn(api, 'listFunctions').mockImplementation(async () => [{ ...current }])
  vi.spyOn(api, 'updateFunction').mockImplementation(async (id, payload) => {
    current = { ...current, content: payload.content, functions: Object.keys(load(payload.content).functions), version: 'f2' }
    return { ...current }
  })
  panel.value = 'function'; await flushPromises()
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  await wrapper.findAll('button').find(b => b.text() === '查看生成 YAML').trigger('click'); await flushPromises()
  expect(Object.keys(load(wrapper.get('.yaml-pre').text()).functions)).toEqual(['chosen'])
  runner.scriptShell.stack.apply({ type: 'insert_param', path: ['functions', 'chosen', 'params'], index: 0,
    decl: { name: 'message', type: 'string', default: 'unsaved visual edit' } })
  await wrapper.findAll('button').find(b => b.text() === '原文编辑').trigger('click'); await flushPromises()
  const raw = wrapper.get('textarea[aria-label="YAML 原文编辑区"]')
  expect(Object.keys(load(raw.element.value).functions)).toEqual(['chosen'])
  expect(load(raw.element.value).functions.chosen.params.message.default).toBe('unsaved visual edit')
  await raw.setValue('functions:\n  renamed:\n    run: []\n')
  await wrapper.findAll('button').find(b => b.text() === action).trigger('click'); await flushPromises()
  expect(Object.keys(load(current.content).functions)).toEqual(['first', 'renamed'])
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('renamed')
  expect(runner.scriptShell.resourceId).toBe(library.id)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  if (action === '运行') expect(run).toHaveBeenCalledWith({ fnName: 'renamed', startIndex: 0 })
})

it('函数原文有未保存修改时切换自动化不会覆盖共享原文', async () => {
  const { panel, runner } = await setup()
  panel.value = 'function'; await flushPromises()
  await wrapper.findAll('button').find(b => b.text() === '原文编辑').trigger('click'); await flushPromises()
  const draft = runner.rawEditor.content.value + '\n# keep draft\n'
  runner.rawEditor.content.value = draft
  panel.value = 'script'; await flushPromises()
  expect(confirm).toHaveBeenCalled()
  expect(runner.rawEditor.content.value).toBe(draft)
  expect(runner.scriptShell.kind).toBe('function_library')
})

it('冲突重载放弃函数改名后恢复同一个已保存函数', async () => {
  const { panel, runner } = await setup()
  panel.value = 'function'; await flushPromises()
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  runner.functionsPanel.renameEditingFunction('chosen', 'renamed')
  await runner.functionsPanel.onConflictReload(); await flushPromises()
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('chosen')
  expect(wrapper.get('select[aria-label="选择函数"]').element.value).toBe(`${library.id}#chosen`)
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

it.each([false, true])('keeps the selected function and undo history after save (conflict: %s)', async conflict => {
  const { panel, runner, getFunction } = await setup()
  panel.value = 'function'; await flushPromises()
  await wrapper.get('select[aria-label="选择函数"]').setValue(`${library.id}#chosen`); await flushPromises()
  const shell = runner.scriptShell
  shell.stack.apply({ type: 'insert_param', path: ['functions', 'chosen', 'params'], index: 0,
    decl: { name: 'message', type: 'string', default: 'hello' } })
  const model = shell.model, stack = shell.stack, loads = getFunction.mock.calls.length
  const save = vi.spyOn(api, 'updateFunction').mockResolvedValue({ ...library, version: 'f2' })
  if (conflict) save.mockRejectedValueOnce(Object.assign(new Error('changed'), { status: 409, data: { code: 'version_conflict' } }))
  await flushPromises()
  await wrapper.findAll('button').find(b => b.text() === '保存').trigger('click'); await flushPromises()
  if (conflict) {
    expect(shell.conflict).toBeTruthy()
    await runner.functionsPanel.onConflictOverwrite(); await flushPromises()
  }
  expect(save).toHaveBeenCalledTimes(conflict ? 2 : 1)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  expect(wrapper.text()).not.toContain('正在加载编辑器')
  expect(wrapper.get('input[aria-label="函数名称"]').element.value).toBe('chosen')
  expect(shell.model).toBe(model)
  expect(shell.stack).toBe(stack)
  expect(shell.dirty).toBe(false)
  expect(shell.version).toBe('f2')
  expect(getFunction).toHaveBeenCalledTimes(loads)
  expect(shell.undo()).toBe(true)
  expect(shell.dirty).toBe(true)
})
