// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import Picker from './workspace/MarketSourcePicker.vue'

const defaults = [
  { id: 'github', name: 'GitHub', repository: 'jesongit/gamer-plugins', enabled: true, builtin: true },
  { id: 'lan', name: '内网', repository: 'http://192.168.31.10:13000/posase/gamer-plugins', enabled: true, builtin: true },
]
function client() {
  let sources = structuredClone(defaults)
  return {
    pluginSources: vi.fn(async () => structuredClone(sources)),
    savePluginSource: vi.fn(async (repository, enabled, preset) => {
      const source = { id: preset.id || 'custom', name: preset.name, repository, enabled, builtin: false }
      sources = [...sources.filter(s => s.id !== source.id), source]
      return structuredClone(sources)
    }),
    removePluginSource: vi.fn(async id => { sources = sources.filter(s => s.id !== id) }),
  }
}
const button = (w, text) => w.findAll('button').find(b => b.text() === text)
beforeEach(() => localStorage.clear())

describe('来源预设', () => {
  it('其他浏览器并发新增预设后，仍选中本次保存的预设', async () => {
    const api=client()
    api.savePluginSource.mockResolvedValue([...defaults,
      {id:'concurrent',name:'其他浏览器的预设',repository:'https://other.example/catalog.json',enabled:true},
      {id:'mine',name:'我的预设',repository:'https://mine.example/registry.json',enabled:true},
    ])
    const w=mount(Picker,{props:{kind:'plugin',apiClient:api}});await flushPromises()
    await button(w,'管理预设').trigger('click')
    await w.get('[aria-label="预设名称"]').setValue('我的预设')
    await w.get('[aria-label="来源 URL"]').setValue('https://mine.example/registry.json')
    await w.get('form').trigger('submit');await flushPromises()
    expect(w.get('select').element.value).toBe('mine')
    expect(w.emitted('change').at(-1)[0].repository).toBe('https://mine.example/registry.json')
    w.unmount()
  })
  it('预设读取失败时锁定管理，重试成功后恢复', async () => {
    const api=client();api.pluginSources.mockRejectedValueOnce(new Error('HTTP 503'))
    const w=mount(Picker,{props:{kind:'plugin',apiClient:api}});await flushPromises()
    expect(button(w,'管理预设').element.disabled).toBe(true)
    await button(w,'重试').trigger('click');await flushPromises()
    expect(button(w,'管理预设').element.disabled).toBe(false)
    w.unmount()
  })
  it('内置下拉来源与浏览器独立记忆；不需要修改服务端默认源', async () => {
    const api = client(), w = mount(Picker, { props: { kind: 'plugin', apiClient: api } }); await flushPromises()
    expect(w.emitted('change')[0][0].id).toBe('github')
    await w.get('select').setValue('lan'); await flushPromises()
    expect(w.emitted('change').at(-1)[0].id).toBe('lan')
    expect(api.savePluginSource).not.toHaveBeenCalled()
    w.unmount()
    const second = mount(Picker, { props: { kind: 'plugin', apiClient: api } }); await flushPromises()
    expect(second.get('select').element.value).toBe('lan')
    expect(localStorage.getItem('gamer.market.package.source')).toBeNull()
    second.unmount()
  })
  it('共享预设新增、重启后读取、编辑、删除均调用服务端，内置项不可删除', async () => {
    const api = client(), w = mount(Picker, { props: { kind: 'plugin', apiClient: api } }); await flushPromises()
    await button(w, '管理预设').trigger('click')
    expect(button(w, '删除当前预设')).toBeUndefined()
    await w.get('[aria-label="预设名称"]').setValue('我的插件')
    await w.get('[aria-label="来源 URL"]').setValue('https://custom.example/registry.json')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(api.savePluginSource).toHaveBeenCalledWith('https://custom.example/registry.json', true, { name: '我的插件' })
    expect(w.get('select').element.value).toBe('custom')
    w.unmount(); localStorage.clear()
    const second = mount(Picker, { props: { kind: 'plugin', apiClient: api } }); await flushPromises()
    expect(second.findAll('option').map(o => o.text())).toContain('我的插件')
    await second.get('select').setValue('custom'); await button(second, '管理预设').trigger('click')
    await button(second, '编辑当前预设').trigger('click')
    await second.get('[aria-label="预设名称"]').setValue('改名')
    await second.get('form').trigger('submit'); await flushPromises()
    expect(api.savePluginSource).toHaveBeenLastCalledWith('https://custom.example/registry.json', true, { id: 'custom', name: '改名' })
    await button(second, '删除当前预设').trigger('click'); await flushPromises()
    expect(api.removePluginSource).toHaveBeenCalledWith('custom')
    expect(second.get('select').element.value).toBe('github')
    second.unmount()
  })
  it('保存失败保留输入与原选择，允许用户纠正地址', async () => {
    const api = client(); api.savePluginSource.mockRejectedValue(new Error('来源须使用 HTTPS'))
    const w = mount(Picker, { props: { kind: 'plugin', apiClient: api } }); await flushPromises()
    await button(w, '管理预设').trigger('click')
    await w.get('[aria-label="预设名称"]').setValue('错误源')
    await w.get('[aria-label="来源 URL"]').setValue('http://public.example/repo')
    await w.get('form').trigger('submit'); await flushPromises()
    expect(w.get('[role="alert"]').text()).toContain('HTTPS')
    expect(w.get('[aria-label="来源 URL"]').element.value).toBe('http://public.example/repo')
    expect(w.get('select').element.value).toBe('github')
    w.unmount()
  })
})
