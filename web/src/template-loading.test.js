// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import { useConsoleTemplates } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })

function setup(packageId) {
  const templatesData = ref([])
  let panel
  wrapper = mount(defineComponent({ setup() {
    panel = useConsoleTemplates({
      packageId, templatesData, toast: vi.fn(), store: { deviceId: null },
      connected: ref(false), videoElement: ref(null), videoWrap: ref(null), current: ref(null),
    })
    return () => h('div', panel.templates.value.map(t => h('span', t.name)))
  } }))
  return { panel, templatesData }
}

it('刷新页面后 Package 延迟恢复，已有模板自动显示', async () => {
  const list = vi.spyOn(api, 'listTemplates').mockResolvedValue([{ pkg: 'game', name: '保留的模板.png' }])
  const pkg = ref(null)
  setup(pkg)
  await flushPromises()
  expect(list).not.toHaveBeenCalled()
  pkg.value = 'game'
  await flushPromises()
  expect(list).toHaveBeenCalledWith('game')
  expect(wrapper.text()).toContain('保留的模板.png')
})

it('切换 Package 后，旧请求迟到不能清空或覆盖新模板列表', async () => {
  let resolveOld
  vi.spyOn(api, 'listTemplates').mockImplementation(pkg => pkg === 'old'
    ? new Promise(resolve => { resolveOld = resolve })
    : Promise.resolve([{ pkg: 'new', name: '当前模板.png' }]))
  const pkg = ref('old')
  const { templatesData } = setup(pkg)
  pkg.value = 'new'
  await flushPromises()
  expect(wrapper.text()).toContain('当前模板.png')
  resolveOld([{ pkg: 'old', name: '旧模板.png' }])
  await flushPromises()
  expect(wrapper.text()).toContain('当前模板.png')
  expect(templatesData.value).toEqual([{ pkg: 'new', name: '当前模板.png' }])
})
