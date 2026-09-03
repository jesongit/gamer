// @vitest-environment happy-dom
// PluginCenter 已安装页签的版本切换（回滚）UI：非 active 版本提供
// 「切换到此版本」入口，确认后走 activate 契约；409（Running）转友好提示。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import PluginCenter from './workspace/plugin-center/PluginCenter.vue'
import { activateVersionErrorText, activateVersionPrompt } from './workspace/plugin-center/plugin-service'

vi.mock('./api', () => ({ api: { activateExtension: vi.fn() } }))
import { api } from './api'

function jsonResponse(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: name => (/content-type/i.test(name) ? 'application/json' : null) },
    json: async () => body,
  }
}

const INSTALLED = {
  extensions: [{
    id: 'official.vision', name: 'Vision', version: '3.0.0', active_version: '3.0.0',
    state: 'enabled', source: 'official', publisher: 'Gamer',
    installed_versions: ['3.0.0', '2.9.0', '2.8.1'],
    permissions: [], signature: { status: 'valid' },
  }],
}

async function mountCenter(overrides = {}) {
  const apiClient = {
    getExtensionManagement: vi.fn().mockResolvedValue(structuredClone(INSTALLED)),
    activateExtension: vi.fn().mockResolvedValue({ id: 'official.vision', active_version: '2.9.0', state: 'enabled' }),
    ...overrides,
  }
  const wrapper = mount(PluginCenter, {
    props: { open: true, apiClient },
    global: { stubs: { Teleport: true } },
  })
  await flushPromises()
  // 切到「已安装」页签
  await wrapper.findAll('.plugin-center-tabs button')[1].trigger('click')
  await flushPromises()
  return { wrapper, apiClient }
}

describe('PluginCenter 版本切换（回滚）', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(200, { schema_version: 1, plugins: [] })))
    vi.stubGlobal('confirm', vi.fn(() => true))
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    api.activateExtension.mockClear()
  })

  it('历史版本行只为非 active 版本提供「切换到此版本」按钮', async () => {
    const { wrapper } = await mountCenter()
    const items = wrapper.findAll('.version-switch-item')
    expect(items).toHaveLength(2)
    const codes = items.map(item => item.find('code').text())
    expect(codes).toEqual(['2.9.0', '2.8.1'])
    expect(wrapper.text()).not.toMatch(/切换到此版本[\s\S]*3\.0\.0[\s\S]*切换到此版本/)
    for (const item of items) {
      expect(item.find('button').text()).toBe('切换到此版本')
    }
  })

  it('确认后按 activate 契约调用并刷新插件状态', async () => {
    const { wrapper, apiClient } = await mountCenter()
    await wrapper.findAll('.version-switch-item button')[0].trigger('click')
    await flushPromises()

    expect(globalThis.confirm).toHaveBeenCalledTimes(1)
    const message = vi.mocked(globalThis.confirm).mock.calls[0][0]
    expect(message).toContain('2.9.0')
    expect(message).toContain('Vision')

    expect(apiClient.activateExtension).toHaveBeenCalledTimes(1)
    expect(apiClient.activateExtension).toHaveBeenCalledWith('official.vision', '2.9.0')
    // 成功提示带目标版本；changed 事件通知宿主；管理状态重新拉取
    expect(wrapper.find('.plugin-alert.info').text()).toContain('2.9.0')
    expect(wrapper.emitted('changed')).toBeTruthy()
    expect(apiClient.getExtensionManagement).toHaveBeenCalledTimes(2)
  })

  it('确认弹窗取消时不发起 activate', async () => {
    vi.stubGlobal('confirm', vi.fn(() => false))
    const { apiClient } = await mountCenter()
    expect(apiClient.activateExtension).not.toHaveBeenCalled()
  })

  it('409（插件 Running）显示「先停止再切换」的行动指引', async () => {
    const conflict = Object.assign(new Error('plugin is running'), { status: 409 })
    const { wrapper } = await mountCenter({ activateExtension: vi.fn().mockRejectedValue(conflict) })
    await wrapper.findAll('.version-switch-item button')[0].trigger('click')
    await flushPromises()

    const alert = wrapper.find('.plugin-alert.error')
    expect(alert.exists()).toBe(true)
    expect(alert.text()).toContain('先停止插件再切换版本')
    expect(wrapper.emitted('changed')).toBeFalsy()
  })

  it('plugin-service 的确认文案与错误映射（409/404/透传）', () => {
    expect(activateVersionPrompt({ id: 'official.vision', name: 'Vision', active_version: '3.0.0' }, '2.9.0'))
      .toMatch(/Vision.*3\.0\.0.*2\.9\.0/s)
    expect(activateVersionErrorText(Object.assign(new Error('x'), { status: 409 }))).toContain('先停止')
    expect(activateVersionErrorText(Object.assign(new Error('x'), { status: 404 }))).toContain('未安装')
    expect(activateVersionErrorText(new Error('网络请求失败'))).toBe('网络请求失败')
  })
})
