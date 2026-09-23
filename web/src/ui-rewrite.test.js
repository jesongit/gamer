// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { createOperationFeedback } from './workspace/operation-feedback'
import { createUiBridge } from './workspace/bridge'
import { useConsolePanelResize } from './components/console/useConsolePanelResize'
import { useScriptEditorShell } from '../../plugins/gamer-yaml/ui/src/composables/useScriptEditorShell'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import { api } from './api'
import { scriptsData, templatesData } from './store'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'
import catalog from '../../tools/yaml-tests/native-functions.json'
import PluginWorkspace from './workspace/PluginWorkspace.vue'
import { createPanelRegistry } from './workspace/registry'
import OperationStatusBar from './workspace/OperationStatusBar.vue'
import { currentPackageId, packageStore, selectPackage } from './package-store'
import { cellShort, parseStepPath } from '../../plugins/gamer-yaml/ui/src/script-editor/components/kinds'

let wrapper
beforeEach(() => { vi.spyOn(api, 'listRunHistory').mockResolvedValue([]) })

it('步骤摘要显示完整复合值，错误路径支持中文函数与宿主 functions 前缀', () => {
  expect(cellShort({ lit: { value: true } })).toBe('{"value":true}')
  expect(cellShort({ lit: [1, 2, 3] })).toBe('[1,2,3]')
  expect(parseStepPath('functions.每日任务.run[0].then[1]')).toEqual(['functions', '每日任务', 'run', 0, 'then', 1])
  expect(parseStepPath('每日任务.run[0]')).toEqual(['functions', '每日任务', 'run', 0])
})
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks(); scriptsData.value = []; templatesData.value = []; localStorage.clear() })

it('只读 Package 上下文被外部切换时，未保存草稿阻止切换且持久化仍是原包', async () => {
  vi.spyOn(api, 'listScripts').mockResolvedValue([])
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: [] })
  packageStore.packages = [{ id: 'one' }, { id: 'two' }]
  selectPackage('one')
  let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: currentPackageId, restorePackage: selectPackage, toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: name => name, loadData: vi.fn() })
    return () => h('div')
  } }))
  runner.scriptShell.newScript({ name: 'draft.yaml', pkg: 'one' })
  runner.scriptShell.scriptDisplayName = 'keep-me'
  selectPackage('two')
  await flushPromises()
  expect(currentPackageId.value).toBe('one')
  expect(localStorage.getItem('gamer.currentPackageId')).toBe('one')
  expect(runner.scriptShell.name).toBe('keep-me.yaml')
  wrapper.unmount(); wrapper = null
  packageStore.currentPackageId = null; packageStore.packages = []
})

it('冷启动时路由指定的插件稍后注册，内容自动出现；卸载后清空', async () => {
  const registry = createPanelRegistry()
  wrapper = mount(PluginWorkspace, { props: { registry, activePanel: 'example.plugin:editor' }, global: { stubs: { teleport: true, PluginCenter: true } } })
  expect(wrapper.text()).toContain('没有可用面板')
  const remove = registry.register({ pluginId: 'example.plugin', panelId: 'editor', title: '编辑', location: 'console.right', runtime: 'core', component: defineComponent({ template: '<div>EDITOR_READY</div>' }) })
  await flushPromises()
  expect(wrapper.text()).toContain('EDITOR_READY')
  remove(); await flushPromises()
  expect(wrapper.text()).not.toContain('EDITOR_READY')
})

it('状态条复制真实数据并显示成功，键盘分界受限且不修改反馈内容', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined)
  vi.spyOn(navigator, 'clipboard', 'get').mockReturnValue({ writeText })
  wrapper = mount(OperationStatusBar, { props: { coreStatuses: ['键盘控制已启用', '60 fps · 12 ms · 4 Mbps'], core: { text: '点击了', actions: [{ label: '(300, 500)', copy: '[300, 500]' }] }, plugin: { text: '未保存', actions: [] } } })
  expect(wrapper.get('.core-zone').attributes('title')).toContain('点击了 · (300, 500) · 键盘控制已启用 · 60 fps · 12 ms · 4 Mbps')
  await wrapper.setProps({ coreStatuses: ['59 fps · 15 ms · 3 Mbps'] })
  expect(wrapper.text()).not.toContain('键盘控制已启用')
  expect(wrapper.get('.core-zone').text()).toContain('点击了')
  expect(wrapper.get('.plugin-zone').text()).toBe('未保存')
  await wrapper.get('.operation-content button').trigger('click'); await flushPromises()
  expect(writeText).toHaveBeenCalledWith('[300, 500]')
  expect(wrapper.text()).toContain('已复制')
  await wrapper.get('[role="separator"]').trigger('keydown', { key: 'End' })
  expect(wrapper.get('[role="separator"]').attributes('aria-valuenow')).toBe('80')
  await wrapper.get('[role="separator"]').trigger('dblclick')
  expect(wrapper.get('[role="separator"]').attributes('aria-valuenow')).toBe('50')
  await wrapper.setProps({ coreStatuses: [] })
  expect(wrapper.find('.core-status').exists()).toBe(false)
})

it('操作反馈按插件隔离，只保留当前一条，限制长文本和动作数量', () => {
  const feedback = createOperationFeedback()
  feedback.setCore({ text: '点击了', actions: [{ label: '(300, 500)', copy: '[300, 500]' }] })
  feedback.setPlugin('one', { text: '未保存' })
  feedback.setPlugin('two', { text: 'a'.repeat(2000), actions: Array.from({ length: 8 }, () => ({ label: '复制', copy: 'value' })) })
  feedback.setPlugin('one', { text: '已保存' })
  expect(feedback.state.core.actions[0].copy).toBe('[300, 500]')
  expect(feedback.state.plugins.one.text).toBe('已保存')
  expect(feedback.state.plugins.two.text).toHaveLength(1000)
  expect(feedback.state.plugins.two.actions).toHaveLength(4)
  feedback.clearPlugin('one')
  expect(feedback.state.plugins.one).toBeUndefined()
  expect(feedback.state.plugins.two).toBeTruthy()
})

it('iframe 状态身份取宿主元数据，拒绝匿名调用，不接受可执行 action', async () => {
  const operationStatus = vi.fn()
  const bridge = createUiBridge({ operationStatus })
  await expect(bridge.dispatch('status.set', { text: '未保存' })).rejects.toMatchObject({ code: 'scope_required' })
  await bridge.dispatch('status.set', { text: '完成', pluginId: 'spoofed', actions: [{ label: '复制', copy: 'abc', run: () => {} }] }, { pluginId: 'real', panelId: 'editor' })
  expect(operationStatus).toHaveBeenCalledWith({ text: '完成', tone: undefined, actions: [{ label: '复制', copy: 'abc' }] }, { pluginId: 'real', panelId: 'editor' })
  await expect(bridge.dispatch('status.set', { text: 'a'.repeat(1001) }, { pluginId: 'real', panelId: 'editor' })).rejects.toMatchObject({ code: 'invalid_request' })
})

it('主分隔条默认 54:46，右拖按画面高度限制在 16:9，保留编辑器最小宽度', async () => {
  let resize
  const container = { clientWidth: 1920 }
  const video = { clientHeight: 800 }
  wrapper = mount(defineComponent({ setup() { resize = useConsolePanelResize({ consoleEl: ref(container), videoWrap: ref(video) }); return () => h('div') } }))
  expect(resize.panelWidth.value).toBe(Math.round(1914 * .46))
  resize.startPanelResize({ button: 0, clientX: 1000, pointerId: 1, preventDefault() {} })
  resize.onPanelResize({ clientX: -5000, pointerId: 1 })
  expect(resize.panelWidth.value).toBe(957)
  resize.stopPanelResize({ pointerId: 1 })
  container.clientWidth = 1500
  window.dispatchEvent(new Event('resize'))
  expect(resize.panelWidth.value).toBe(747)
  resize.startPanelResize({ button: 0, clientX: 1000, pointerId: 2, preventDefault() {} })
  resize.onPanelResize({ clientX: 5000, pointerId: 2 })
  expect(resize.panelWidth.value).toBe(320)
  resize.stopPanelResize({ pointerId: 2 })
  container.clientWidth = 1920
  window.dispatchEvent(new Event('resize'))
  expect(resize.panelWidth.value).toBe(492)
  expect(1914 - resize.panelWidth.value).toBeLessThanOrEqual(800 * 16 / 9)
  video.clientHeight = 700
  window.dispatchEvent(new Event('resize'))
  expect(resize.panelWidth.value).toBe(670)
  // 切至管理页时舞台隐藏，不应把尺寸/偏好重置为零。
  video.clientHeight = 0
  window.dispatchEvent(new Event('resize'))
  expect(resize.panelWidth.value).toBe(670)
  video.clientHeight = 1000
  window.dispatchEvent(new Event('resize'))
  expect(resize.PANEL_MIN_WIDTH.value).toBe(320)
  resize.onPanelResizeKeydown({ key: 'ArrowRight', preventDefault() {} })
  expect(resize.panelWidth.value).toBeGreaterThanOrEqual(320)
})

it('快速切换资源时，迟到的脚本响应不会覆写新的函数或新建内容', async () => {
  let resolveScript
  const shell = useScriptEditorShell({ api: {
    getScript: () => new Promise(resolve => { resolveScript = resolve }),
    getFunction: async () => ({ id: 'qa/_function.yaml', file: '_function.yaml', pkg: 'qa', content: 'functions:\n  demo:\n    run: []\n' }),
  } })
  const pending = shell.loadScript('qa/old.yaml')
  await shell.loadFunctionFile('qa/_function.yaml')
  resolveScript({ id: 'qa/old.yaml', name: 'old.yaml', content: 'run: []\n' })
  await pending
  expect(shell.kind).toBe('function_library')
  expect(shell.resourceId).toBe('qa/_function.yaml')
  const stale = shell.loadScript('qa/old.yaml')
  shell.newScript({ name: 'new.yaml', pkg: 'qa' })
  resolveScript({ id: 'qa/old.yaml', name: 'old.yaml', content: 'run: []\n' })
  await stale
  expect(shell.name).toBe('new.yaml')
  expect(shell.resourceId).toBeNull()
})

it('手动保存后保留内联画布和命令栈，实际保存仍携带资源版本', async () => {
  const source = { id: 'qa/main.yaml', package: 'qa', name: 'main.yaml', version: 'v1', content: 'run:\n  - return: true\n' }
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: catalog })
  vi.spyOn(api, 'listScripts').mockResolvedValue([source])
  vi.spyOn(api, 'getScript').mockResolvedValue(source)
  const save = vi.spyOn(api, 'updateScript').mockImplementation(async (id, payload) => { Object.assign(source, payload, { id: `qa/${payload.name}`, version: 'v2' }); return source })
  let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: ref('qa'), toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: name => name, loadData: vi.fn() })
    return () => h(ScriptRunner, { context: runner.scriptPanel })
  } }))
  await flushPromises()
  const shell = runner.scriptShell
  const nameInput = wrapper.get('.editor-toolbar input[aria-label="脚本名称"]')
  expect(nameInput.element.nextElementSibling.classList.contains('add-step')).toBe(true)
  expect(nameInput.element.value).toBe('main')
  await nameInput.setValue('renamed')
  const stack = shell.stack
  // 不依赖 blur：输入后直接点击保存也会先提交名称。
  const saveButton = wrapper.findAll('.resource-action').find(button => button.text() === '保存')
  expect(saveButton.element.disabled).toBe(false)
  await saveButton.trigger('click')
  await flushPromises()
  expect(save.mock.calls[0][1].name).toBe('renamed.yaml')
  expect(save.mock.calls[0][1].expected_version).toBe('v1')
  expect(shell.stack).toBe(stack)
  expect(shell.dirty).toBe(false)
  expect(wrapper.find('.se-canvas').exists()).toBe(true)
  await nameInput.setValue('')
  await saveButton.trigger('click')
  expect(save).toHaveBeenCalledTimes(1)
  expect(wrapper.get('[role="alert"]').text()).toBe('名称不能为空')
  await nameInput.trigger('keydown', { key: 'Escape' })
  expect(nameInput.element.value).toBe('renamed')
})

it('从此运行在保存后继续使用编辑器步骤序号，冲突时不发起运行', async () => {
  const runScript = vi.fn(), saveEditScript = vi.fn()
  const context = reactive({ runKind: 'script', scriptMode: 'edit', packageId: 'qa', selScript: 'qa/main.yaml', filteredFnViews: [], templateNames: [],
    store: { running: false, deviceId: 'device' }, shell: { hasModel: true, kind: 'script', dirty: true, resourceId: 'qa/main.yaml', diagnostics: [], model: { run: [{ uuid: 'first' }, { uuid: 'second' }] }, stack: {} },
    saveEditScript, runScript, raw: {}, resourcePreview: {}, runFromStep: vi.fn(),
  })
  saveEditScript.mockImplementationOnce(async () => { context.shell.dirty = false; return { ok: true } })
  wrapper = mount(ScriptRunner, { props: { context }, global: { stubs: { ScriptPicker: true, StepCanvas: { template: '<button class="test-from-stub" @click="$emit(\'test-from\', \'second\')">run</button>' }, ParamEditor: true, ResourcePreviewModal: true, SaveConflictModal: true } } })
  await wrapper.get('.test-from-stub').trigger('click'); await flushPromises()
  expect(runScript).toHaveBeenCalledWith({ startIndex: 1 })
  context.shell.dirty = true
  saveEditScript.mockResolvedValueOnce({ ok: false, reason: 'conflict' })
  await wrapper.get('.test-from-stub').trigger('click'); await flushPromises()
  expect(runScript).toHaveBeenCalledTimes(1)
})

it.each(['script', 'func'])('顶部运行按钮从第零步运行 %s，不把点击事件当作步骤 UUID', async runKind => {
  const runScript = vi.fn(), runFunction = vi.fn(), saveEditScript = vi.fn()
  const model = runKind === 'func'
    ? { functions: [{ name: '账号日常', params: [], run: [{ uuid: 'first' }] }] }
    : { run: [{ uuid: 'first' }] }
  const context = reactive({ runKind, scriptMode: 'edit', packageId: 'qa', selScript: 'qa/main.yaml', editFocusFn: '账号日常', filteredFnViews: [], templateNames: [],
    store: { running: false, deviceId: 'device' },
    shell: { hasModel: true, kind: runKind === 'func' ? 'function_library' : 'script', dirty: true, resourceId: 'qa/main.yaml', diagnostics: [], model, stack: {}, scriptDisplayName: '账号日常' },
    saveEditScript, runScript, runFunction, raw: {}, resourcePreview: {},
  })
  saveEditScript.mockImplementation(async () => { context.shell.dirty = false; return { ok: true } })
  wrapper = mount(ScriptRunner, { props: { context }, global: { stubs: { ScriptPicker: true, StepCanvas: true, ParamEditor: true, ResourcePreviewModal: true, SaveConflictModal: true } } })
  const button = wrapper.get('.resource-toolbar .btn-primary')
  expect(button.element.disabled).toBe(false)
  await button.trigger('click')
  await flushPromises()
  const run = runKind === 'func' ? runFunction : runScript
  expect(run).toHaveBeenCalledExactlyOnceWith(runKind === 'func' ? { fnName: '账号日常', startIndex: 0 } : { startIndex: 0 })
  expect(saveEditScript.mock.invocationCallOrder[0]).toBeLessThan(run.mock.invocationCallOrder[0])
  context.shell.dirty = true
  saveEditScript.mockResolvedValueOnce({ ok: false, reason: 'conflict' })
  await button.trigger('click')
  await flushPromises()
  expect(run).toHaveBeenCalledTimes(1)
})


it('运行被接受后用详情占满编辑区，结束后保留，返回时保留编辑器实例', async () => {
  const context = reactive({ runKind:'script',scriptMode:'edit',packageId:'qa',selScript:'qa/main.yaml',filteredFnViews:[],templateNames:[],
    store:{running:false,deviceId:'d',runId:null}, shell:{hasModel:true,kind:'script',dirty:false,resourceId:'qa/main.yaml',diagnostics:[],model:{run:[]},stack:{}},raw:{},resourcePreview:{} })
  wrapper = mount(ScriptRunner,{props:{context},global:{stubs:{ScriptPicker:true,StepCanvas:{template:'<div />',methods:{closeAdd(){}}},ParamEditor:true,ResourcePreviewModal:true,SaveConflictModal:true,RunDetails:{template:'<section class="details-stub"><button @click="$emit(\'edit\')">返回编辑</button></section>'}}}})
  expect(wrapper.find('.details-stub').element.style.display).toBe('none')
  const editor = wrapper.get('.editor-view').element
  context.store.running = true; context.store.runId = 'accepted'; await flushPromises()
  expect(wrapper.find('.details-stub').element.style.display).not.toBe('none')
  expect(editor.style.display).toBe('none')
  context.store.running = false; context.store.runId = null; await flushPromises()
  expect(wrapper.find('.details-stub').element.style.display).not.toBe('none')
  context.selScript = 'qa/another.yaml'; await flushPromises()
  expect(wrapper.find('.details-stub').element.style.display).not.toBe('none')
  await wrapper.get('.details-stub button').trigger('click')
  expect(wrapper.get('.editor-view').element).toBe(editor)
  expect(editor.style.display).not.toBe('none')
})
