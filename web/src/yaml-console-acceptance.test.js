// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { api } from './api'
import { scriptsData, templatesData } from './store'
import { useConsoleScriptRunner } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleScriptRunner'
import ScriptRunner from '../../plugins/gamer-yaml/ui/src/components/console/ScriptRunner.vue'
import catalog from '../../tools/yaml-tests/native-functions.json'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks(); scriptsData.value = []; templatesData.value = [] })

function mountRunner(packageId) {
  let runner
  wrapper = mount(defineComponent({ setup() {
    runner = useConsoleScriptRunner({ packageId, toast: vi.fn(), consoleRuntime: {},
      templateNames: ref([]), tplShortName: name => name, loadData: vi.fn() })
    return () => h(ScriptRunner, { context: runner.scriptPanel })
  } }))
  return runner
}

it('Package 在页面挂载后才加载：脚本列表与摘要自动出现，切换包不保留旧结果', async () => {
  vi.spyOn(api, 'getRunnerFunctions').mockResolvedValue({ functions: catalog })
  vi.spyOn(api, 'listScripts').mockImplementation(async pkg => [{ id: `${pkg}/main.yaml`, package: pkg, name: 'main.yaml', content: 'run:\n  - log: {message: hello, name: hello}\n' }])
  vi.spyOn(api, 'getScript').mockImplementation(async id => ({ id, package: id.split('/')[0], name: 'main.yaml', version: 'v1', content: 'run:\n  - log: {message: hello, name: hello}\n' }))
  const pkg = ref(null)
  mountRunner(pkg)
  await flushPromises()
  pkg.value = 'qa'
  await flushPromises()
  expect(wrapper.get('select[title="运行脚本"]').element.value).toBe('qa/main.yaml')
  expect(wrapper.text()).toContain('hello')
  expect(wrapper.text()).not.toContain('脚本解析失败')
  pkg.value = 'second'
  await flushPromises()
  expect(wrapper.get('select[title="运行脚本"]').element.value).toBe('second/main.yaml')
})

it('首次函数目录不可用：进入编辑会重试，内置函数仍可从菜单加入', async () => {
  const getFunctions = vi.spyOn(api, 'getRunnerFunctions').mockRejectedValueOnce(new Error('runner 未启用')).mockResolvedValue({ functions: catalog })
  vi.spyOn(api, 'listScripts').mockResolvedValue([])
  mountRunner(ref('qa'))
  await flushPromises()
  await wrapper.findAll('button').find(b => b.text() === '新建脚本').trigger('click')
  await flushPromises()
  expect(getFunctions).toHaveBeenCalledTimes(2)
  expect(wrapper.get('input[aria-label="脚本名称"]').element.value).toBe('新脚本')
  await wrapper.get('button[title="添加步骤"]').trigger('click')
  // 添加列表通过 Teleport 脱离编辑区滚动裁切，菜单仍消费同一函数目录。
  expect(document.body.querySelector('[aria-label="调用 wait_find"]')).not.toBeNull()
})
