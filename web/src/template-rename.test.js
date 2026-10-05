// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { api } from './api'
import TemplateCapture from '../../plugins/gamer-yaml/ui/src/components/console/TemplateCapture.vue'
import { useConsoleTemplates } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })

const oldName = '旧模板#253_114_332_211.png'
const newName = '旷宇纷争#253_114_332_211.png'
const target = { pkg: 'game', name: oldName }

function setup() {
  const packageId = ref('game')
  const toast = vi.fn()
  const refreshScripts = vi.fn()
  const refreshFnLib = vi.fn()
  const clearCallParamsCache = vi.fn()
  let panel
  wrapper = mount(defineComponent({ setup() {
    panel = useConsoleTemplates({
      packageId, templatesData: ref([]), toast, store: { deviceId: null },
      connected: ref(false), videoElement: ref(null), videoWrap: ref(null), current: ref(null),
      refreshScripts, refreshFnLib, clearCallParamsCache,
    })
    return () => h(TemplateCapture, { context: panel.templateCaptureContext })
  } }), { attachTo: document.body })
  return { panel, packageId, toast, refreshScripts, refreshFnLib, clearCallParamsCache }
}

it('Enter 重命名后立即更新模板行、缩略图和候选短名，无需重新进入页面', async () => {
  vi.spyOn(api, 'listTemplates').mockResolvedValueOnce([target]).mockResolvedValue([{ ...target, name: newName }])
  const rename = vi.spyOn(api, 'renameTemplate').mockResolvedValue({ path: `templates/${newName}` })
  const { panel, toast, refreshScripts, refreshFnLib, clearCallParamsCache } = setup()
  await flushPromises()
  await wrapper.get('button[aria-haspopup="menu"]').trigger('click')
  await flushPromises()
  const action = [...document.body.querySelectorAll('[role="menuitem"]')].find(el => el.textContent === '重命名')
  action.click()
  await flushPromises()
  await wrapper.get('.rename-input').setValue(newName.replace(/\.png$/, ''))
  await wrapper.get('.rename-input').trigger('keydown', { key: 'Enter' })
  await flushPromises()

  expect(rename).toHaveBeenCalledWith(oldName, newName, 'game')
  expect(wrapper.get('.tpl-row').text()).toContain('旷宇纷争.png')
  expect(wrapper.find('.rename-input').exists()).toBe(false)
  expect(wrapper.get('.tpl-thumb img').attributes('src')).toContain(encodeURIComponent(newName))
  expect(panel.templateNames.value).toEqual(['旷宇纷争.png'])
  expect(refreshScripts).toHaveBeenCalledOnce()
  expect(refreshFnLib).toHaveBeenCalledWith('game')
  expect(clearCallParamsCache).toHaveBeenCalledOnce()
  expect(toast).toHaveBeenLastCalledWith(`模板已重命名为 ${newName}`, 'success')
})

it('重命名已保存但列表刷新失败时提示刷新失败，避免虚报完整成功', async () => {
  vi.spyOn(api, 'listTemplates').mockResolvedValueOnce([target]).mockRejectedValue(new Error('offline'))
  vi.spyOn(api, 'renameTemplate').mockResolvedValue({})
  const { panel, toast } = setup()
  await flushPromises()
  panel.startRename(target)
  panel.renameVal.value = newName
  await panel.confirmRename(target)
  await flushPromises()
  expect(toast).toHaveBeenLastCalledWith(expect.stringContaining('模板列表刷新失败'), 'warn')
})

it('重命名请求期间切换配置包，迟到响应不刷新新包的引用或覆盖模板列表', async () => {
  let completeRename
  const list = vi.spyOn(api, 'listTemplates').mockImplementation(pkg => Promise.resolve(
    pkg === 'game' ? [target] : [{ pkg, name: '新包模板.png' }]))
  const rename = vi.spyOn(api, 'renameTemplate').mockImplementation(() => new Promise(resolve => { completeRename = resolve }))
  const { panel, packageId, refreshScripts, refreshFnLib, clearCallParamsCache } = setup()
  await flushPromises()
  panel.startRename(target)
  panel.renameVal.value = newName
  const pending = panel.confirmRename(target)
  packageId.value = 'other'
  await flushPromises()
  completeRename({})
  await pending
  await flushPromises()
  expect(rename).toHaveBeenCalledWith(oldName, newName, 'game')
  expect(wrapper.get('.tpl-row').text()).toContain('新包模板.png')
  expect(list.mock.calls).toEqual([['game'], ['other']])
  expect(refreshScripts).not.toHaveBeenCalled()
  expect(refreshFnLib).not.toHaveBeenCalled()
  expect(clearCallParamsCache).not.toHaveBeenCalled()
})
