// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { reactive, ref } from 'vue'
import { useConsoleWorkspacePanels } from './components/console/useConsoleWorkspacePanels'

let wrapper
afterEach(() => wrapper?.unmount())
async function setup(panel) {
  const route = reactive({ path: '/console', query: panel ? { panel } : {} })
  const router = { push: vi.fn(async location => { route.query = location.query }), replace: vi.fn(async location => { route.query = location.query }) }
  const active = ref('workbench')
  const tasks = { key: 'gamer.core:tasks' }
  let workspace
  wrapper = mount({ setup() {
    workspace = useConsoleWorkspacePanels({ route, router, activePanelKey: active,
      panelRegistry: { resolve: key => key === tasks.key ? tasks : null },
      serverUiAdapter: { refresh: async () => ({ extensions: [] }), dispose() {} },
      remoteKeymapRunning: ref(false), connected: ref(false), keymap: {},
    })
    return () => null
  } })
  await flushPromises()
  return { workspace, active, route, router }
}
it('没有 panel 的首次访问进入工作台，不跳到任务', async () => {
  const { active, route } = await setup()
  expect(active.value).toBe('workbench')
  expect(route.query.panel).toBe('workbench')
})
it('显式任务直达链接保留，未知页面在扩展发现完成后回工作台', async () => {
  const { active, route, workspace } = await setup('gamer.core:tasks')
  expect(active.value).toBe('gamer.core:tasks')
  route.query = { panel: 'unknown:page' }
  await flushPromises()
  expect(active.value).toBe('unknown:page')
  await workspace.refreshServerExtensions()
  await flushPromises()
  expect(active.value).toBe('workbench')
  expect(route.query.panel).toBe('workbench')
})
it('插件、配置包页面被保留，失效跳转回工作台', async () => {
  const { active, workspace } = await setup('plugins')
  expect(active.value).toBe('plugins')
  await workspace.openPanel('packages')
  expect(active.value).toBe('packages')
  await workspace.openPanel('missing:panel')
  expect(active.value).toBe('workbench')
})
