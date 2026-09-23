// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { createOperationFeedback, OPERATION_FEEDBACK_KEY } from './workspace/operation-feedback'
import OperationStatusBar from './workspace/OperationStatusBar.vue'
import { useOperationStatus } from './components/ui/useOperationStatus'
import { pushRunEvent, useRunEvents } from '../../plugins/gamer-yaml/ui/src/components/console/useRunEvents'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import RunErrorLocation from '../../plugins/gamer-yaml/ui/src/components/console/RunErrorLocation.vue'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'
import { api } from './api'
import { connectGamer, applyGamerTheme } from '../../sdk/ui/gamer-ui.js'

let wrapper
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks() })

it('被裁切状态可悬停展开并操作，Esc 收起，分界位置可恢复', async () => {
  localStorage.removeItem('gamer.operationStatusSplit')
  const copy = vi.fn().mockResolvedValue(undefined)
  vi.spyOn(navigator, 'clipboard', 'get').mockReturnValue({ writeText: copy })
  wrapper = mount(OperationStatusBar, { props: { core: { text: '坐标', actions: [{ label: '(300, 500)', copy: '[300, 500]' }] }, coreStatuses: ['1920x1080 · 60 fps · 20 ms · 4 Mbps'] } })
  const clip = wrapper.get('.core-zone .zone-clip').element
  Object.defineProperties(clip, { clientWidth: { value: 100 }, scrollWidth: { value: 600 } })
  await wrapper.get('.core-zone').trigger('mouseenter')
  expect(wrapper.get('.status-expanded').text()).toContain('1920x1080')
  await wrapper.get('.status-expanded button').trigger('click')
  await flushPromises()
  expect(copy).toHaveBeenCalledWith('[300, 500]')
  await wrapper.get('.core-zone').trigger('keydown', { key: 'Escape' })
  expect(wrapper.find('.status-expanded').exists()).toBe(false)
  await wrapper.get('[role="separator"]').trigger('keydown', { key: 'End' })
  wrapper.unmount()
  wrapper = mount(OperationStatusBar)
  expect(wrapper.get('[role="separator"]').attributes('aria-valuenow')).toBe('80')
  await wrapper.get('[role="separator"]').trigger('dblclick')
  expect(localStorage.getItem('gamer.operationStatusSplit')).toBe('50')
  localStorage.removeItem('gamer.operationStatusSplit')
})

it('状态便捷动作失败显示详情，不成为未处理异常', async () => {
  wrapper = mount(OperationStatusBar, { props: { plugin: { text: '运行报错', actions: [{ label: '跳转', run: async () => { throw new Error('源文件已删除') } }] } }, global: { stubs: { Teleport: true } } })
  await wrapper.get('.operation-content button').trigger('click')
  await flushPromises()
  expect(wrapper.get('.status-detail').text()).toContain('源文件已删除')
  await wrapper.get('.status-detail').trigger('keydown', { key: 'Escape' })
  expect(wrapper.find('.status-detail').exists()).toBe(false)
})

it('同插件旧子面板卸载不会清掉新面板的状态', async () => {
  const feedback = createOperationFeedback(), oldOpen = ref(true), newOpen = ref(false)
  const Child = defineComponent({ props: ['label'], setup(props) { useOperationStatus(() => ({ text: props.label })); return () => h('div') } })
  wrapper = mount(defineComponent({ setup() { return () => [oldOpen.value ? h(Child, { key: 'old', label: '项目未保存' }) : null, newOpen.value ? h(Child, { key: 'new', label: '草稿未保存' }) : null] } }), { global: { provide: { [OPERATION_FEEDBACK_KEY]: feedback, 'gamer-panel-owner': ref('gamer-video') } } })
  newOpen.value = true; await flushPromises()
  expect(feedback.state.plugins['gamer-video'].text).toBe('草稿未保存')
  oldOpen.value = false; await flushPromises()
  expect(feedback.state.plugins['gamer-video'].text).toBe('草稿未保存')
})

it('迟到操作不覆盖新反馈，切换面板后未完成的首个操作不能复活', () => {
  const feedback = createOperationFeedback()
  const first = feedback.begin('plugin')
  const second = feedback.begin('plugin')
  second({ text: '已保存' })
  expect(first({ text: '旧结果' })).toBe(false)
  expect(feedback.state.plugins.plugin.text).toBe('已保存')
  const pending = feedback.begin('new-plugin')
  for (const owner of Object.keys(feedback.state.plugins)) feedback.clearPlugin(owner)
  expect(pending({ text: '迟到结果' })).toBe(false)
})

it('调用退栈保留最内层失败，旧运行事件不会混进新运行', () => {
  const send = (ev, fields = {}) => pushRunEvent({ type: 'se', ev, trace: { run_id: 'new-run' }, ...fields })
  send('run_start')
  send('step_end', { path: '中文.run[2]', ok: false, trace: { run_id: 'new-run', frame_id: 2, source: { path: 'automations/_function_more.yaml', version: 'v2' } } })
  send('step_end', { path: 'run[0]', ok: false })
  expect(useRunEvents().errorPath).toBe('中文.run[2]')
  expect(useRunEvents().errorEvent.trace.frame_id).toBe(2)
  send('step_start', { path: 'run[9]', trace: { run_id: 'old-run' } })
  expect(useRunEvents().activePath).not.toBe('run[9]')
  send('run_start')
  expect(useRunEvents().errorEvent).toBeNull()
})

it('拆分库的函数打开共享画布，保存使用所属文件和读取版本', async () => {
  const file = { id: 'qa/_function_extra.yaml', pkg: 'qa', file: '_function_extra.yaml', version: 'file-v1', functions: ['other', 'helper'], content: 'functions:\n  other:\n    run: []\n  helper:\n    run: []\n' }
  vi.spyOn(api, 'listFunctions').mockResolvedValue([file])
  vi.spyOn(api, 'getFunction').mockResolvedValue(file)
  vi.spyOn(api, 'listScripts').mockResolvedValue([])
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: [] })
  const save = vi.spyOn(api, 'updateFunction').mockResolvedValue({ ...file, version: 'file-v2' })
  let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId: ref('qa'), toast: vi.fn(), consoleRuntime: {}, templateNames: ref([]), tplShortName: value => value, loadData: vi.fn() })
    return () => h(ScriptRunner, { context: runner.functionsPanel })
  } }))
  await runner.fnLib.refresh('qa')
  await runner.functionsPanel.editFunction({ fileId: file.id, name: 'helper', category: '_function_extra' })
  expect(runner.scriptShell.resourceId).toBe(file.id)
  expect(runner.functionsPanel.editFocusFn.value).toBe('helper')
  await flushPromises()
  const nameInput = wrapper.get('.editor-toolbar input[aria-label="函数名称"]')
  expect(nameInput.element.value).toBe('helper')
  await nameInput.setValue('renamed')
  await nameInput.trigger('keydown', { key: 'Enter' })
  expect(runner.functionsPanel.editFocusFn.value).toBe('renamed')
  runner.scriptShell.undo()
  await flushPromises()
  expect(nameInput.element.value).toBe('helper')
  runner.scriptShell.redo()
  await flushPromises()
  expect(nameInput.element.value).toBe('renamed')
  await nameInput.setValue('other')
  await nameInput.trigger('blur')
  expect(nameInput.attributes('aria-invalid')).toBe('true')
  expect(runner.functionsPanel.editFocusFn.value).toBe('renamed')
  await nameInput.trigger('keydown', { key: 'Escape' })
  await wrapper.findAll('.resource-action').find(button => button.text() === '保存').trigger('click')
  await flushPromises()
  expect(save).toHaveBeenCalledWith(file.id, expect.objectContaining({ expected_version: 'file-v1', content: expect.stringContaining('renamed') }))
})

it('报错位置读取真实定义文件，版本变化时拒绝套用旧步骤路径', async () => {
  const get = vi.spyOn(api, 'getFunction').mockResolvedValue({ version: 'changed', content: 'functions: {}' })
  wrapper = mount(RunErrorLocation, { props: { event: { path: 'helper.run[0]', error: '失败', trace: { run_id: 'r', source: { package_id: 'qa', path: 'automations/_function_more.yaml', version: 'executed', function: 'helper' } } } }, global: { stubs: { Teleport: true } } })
  await flushPromises()
  expect(get).toHaveBeenCalledWith('qa/_function_more.yaml')
  expect(wrapper.text()).toContain('源文件已变化')
  expect(wrapper.find('.se-canvas').exists()).toBe(false)
})

it('iframe SDK 使用专属端口、请求 ID、宿主主题与关闭清理', async () => {
  const client = connectGamer({ timeout: 1000 })
  const port = { start: vi.fn(), close: vi.fn(), postMessage: vi.fn() }
  window.dispatchEvent(new MessageEvent('message', { source: window.parent, data: { type: 'gamer-ui:connect', version: 'gamer-ui@1' }, ports: [port] }))
  const result = client.call('context.get')
  await flushPromises()
  const request = port.postMessage.mock.calls[0][0]
  expect(request.method).toBe('context.get')
  port.onmessage({ data: { type: 'gamer-ui:response', version: 'gamer-ui@1', id: request.id, ok: true, result: { theme: { accent: '#e4c956' } } } })
  applyGamerTheme((await result).theme)
  expect(document.documentElement.style.getPropertyValue('--gamer-accent')).toBe('#e4c956')
  client.dispose()
  expect(port.close).toHaveBeenCalledOnce()
})
