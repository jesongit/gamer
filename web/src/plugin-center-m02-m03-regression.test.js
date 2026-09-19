// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
const { dialogDecision } = vi.hoisted(() => ({ dialogDecision: vi.fn() }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => Object.assign(dialogDecision, { cancel: vi.fn() }) }))
import { flushPromises, mount } from '@vue/test-utils'
import PluginCenter from './workspace/plugin-center/PluginCenter.vue'
import {
  describeExtensionMutation,
  marketVersionLabel,
  marketVersionRelation,
} from './workspace/plugin-center/plugin-service'

const sha256 = 'a'.repeat(64)

function entry(id, version, execution = { kind: 'wasm' }) {
  return {
    id,
    version,
    name: id,
    download_url: `/plugins/${id}-${version}.gplugin`,
    sha256,
    execution,
  }
}

function installed(id, version, extra = {}) {
  return {
    id,
    name: id,
    active_version: version,
    installed_versions: [version],
    state: 'disabled',
    execution: { kind: 'wasm' },
    ...extra,
  }
}

function management(extensions) {
  return { schema_version: 1, host_api: '1.0.0', runtime_available: true, extensions, dependencies: {} }
}

function makeClient(extensions) {
  let current = extensions
  const client = {
    getExtensionManagement: vi.fn(async () => management(current)),
    enableExtension: vi.fn(async id => {
      current = current.map(plugin => plugin.id === id ? { ...plugin, state: 'running' } : plugin)
      return current.find(plugin => plugin.id === id)
    }),
    disableExtension: vi.fn(async id => {
      current = current.map(plugin => plugin.id === id ? { ...plugin, state: 'disabled' } : plugin)
      return current.find(plugin => plugin.id === id)
    }),
    updateExtension: vi.fn(),
    installExtension: vi.fn(),
    inspectExtension: vi.fn(),
    uninstallExtension: vi.fn(async id => {
      current = current.filter(plugin => plugin.id !== id)
      // The real API resolves a 204 response without a JSON body.
      return { ok: true, status: 204 }
    }),
  }
  return { client, getCurrent: () => current }
}

function registryFetch(plugins) {
  return vi.fn(async () => ({
    ok: true,
    status: 200,
    headers: { get: () => 'application/json' },
    json: async () => ({ schema_version: 2, plugins }),
  }))
}

async function mountCenter(plugins, extensions) {
  vi.stubGlobal('fetch', registryFetch(plugins))
  const { client } = makeClient(extensions)
  const wrapper = mount(PluginCenter, {
    props: { apiClient: client },
    global: { stubs: { Teleport: { template: '<div><slot /></div>' } } },
  })
  await flushPromises()
  return { wrapper, client }
}

describe('P2-UI M02/M03 插件中心回归', () => {
  beforeEach(() => {
    dialogDecision.mockResolvedValue(true)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('用明确关系区分更新、已是最新、已安装更高版本和执行形态不兼容', () => {
    const same = installed('demo.same', '1.0.0')
    const newer = installed('demo.newer', '2.0.0')
    const older = installed('demo.older', '0.9.0')
    const builtin = installed('demo.builtin', '1.0.0', { execution: { kind: 'builtin' } })

    expect(marketVersionRelation(entry('demo.same', '1.0.0'), same).kind).toBe('latest')
    expect(marketVersionRelation(entry('demo.newer', '1.0.0'), newer).kind).toBe('newer_installed')
    expect(marketVersionRelation(entry('demo.older', '1.0.0'), older).kind).toBe('update')
    expect(marketVersionRelation(entry('demo.builtin', '2.0.0'), builtin).kind).toBe('incompatible')
    expect(marketVersionLabel(marketVersionRelation(entry('demo.same', '1.0.0'), same))).toBe('已是最新')
    expect(marketVersionLabel(marketVersionRelation(entry('demo.newer', '1.0.0'), newer))).toBe('已安装更高版本')
  })

  it('市场卡片不把同版、低版或不兼容版本误显示为可更新', async () => {
    const { wrapper } = await mountCenter([
      entry('demo.same', '1.0.0'),
      entry('demo.newer', '1.0.0'),
      entry('demo.older', '1.0.0'),
      entry('demo.builtin', '2.0.0'),
    ], [
      installed('demo.same', '1.0.0'),
      installed('demo.newer', '2.0.0'),
      installed('demo.older', '0.9.0'),
      installed('demo.builtin', '1.0.0', { execution: { kind: 'builtin' } }),
    ])
    try {
      const cards = wrapper.findAll('.plugin-card')
      expect(cards[0].text()).toContain('已是最新')
      expect(cards[0].findAll('button').some(button => button.text().includes('更新到'))).toBe(false)
      expect(cards[1].text()).toContain('已安装更高版本')
      expect(cards[1].text()).not.toContain('更新到')
      expect(cards[2].text()).toContain('更新到 1.0.0')
      expect(cards[2].find('button').attributes('disabled')).toBeUndefined()
      expect(cards[3].text()).toContain('版本不兼容')
      expect(cards[3].text()).toContain('宿主预置插件不能替换为 WASM 包')
      expect(cards[3].findAll('button').some(button => button.text().includes('更新到'))).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })

  it('操作期间按钮不可重入，完成刷新后成功提示仍保留', async () => {
    const deferred = {}
    const { wrapper, client } = await mountCenter(
      [],
      [installed('demo.action', '1.0.0')],
    )
    client.enableExtension.mockImplementationOnce(() => new Promise(resolve => { deferred.resolve = resolve }))
    try {
      // The installed tab has one lifecycle button; the first click starts the
      // request and the second click must be ignored while it is pending.
      const action = wrapper.findAll('button').find(button => button.text() === '启用')
      expect(action).toBeTruthy()
      await action.trigger('click')
      const pendingAction = wrapper.findAll('button').find(button => button.text() === '启用')
      expect(pendingAction.attributes('disabled')).toBeDefined()
      await pendingAction.trigger('click')
      expect(client.enableExtension).toHaveBeenCalledTimes(1)

      deferred.resolve({ ...installed('demo.action', '1.0.0'), state: 'running' })
      await flushPromises()
      expect(wrapper.text()).toContain('demo.action：已启用')

      await wrapper.find('.plugin-toolbar button').trigger('click')
      await flushPromises()
      expect(wrapper.text()).toContain('demo.action：已启用')
    } finally {
      wrapper.unmount()
    }
  })

  it('同 ID 只显示一张卡片并选择最新市场版本，本地独有插件仍可管理', async () => {
    const { wrapper } = await mountCenter([entry('demo.alpha', '1.0.0'), entry('demo.alpha', '2.0.0'), entry('demo.beta', '1.0.0')], [installed('demo.alpha', '1.0.0'), installed('demo.local', '1.0.0')])
    try {
      expect(wrapper.findAll('.plugin-card')).toHaveLength(3)
      expect(wrapper.findAll('.installed-card')).toHaveLength(2)
      expect(wrapper.get('[data-plugin-id="demo.alpha"]').text()).toContain('更新到 2.0.0')
      expect(wrapper.get('[data-plugin-id="demo.local"]').text()).toContain('未收录于市场')
      expect(wrapper.get('[data-plugin-id="demo.beta"]').text()).toContain('未安装')
      expect(wrapper.find('.plugin-section').exists()).toBe(false)
      await wrapper.get('[aria-label="搜索插件"]').setValue('alpha')
      expect(wrapper.findAll('.plugin-card')).toHaveLength(1)
      await wrapper.findAll('button').find(button => button.text() === 'URL 导入').trigger('click')
      expect(wrapper.find('input[type="url"]').exists()).toBe(true)
      expect(wrapper.findAll('.plugin-card')).toHaveLength(1)
      await wrapper.get('[aria-label="搜索插件"]').setValue('')
      expect(wrapper.findAll('.plugin-card')).toHaveLength(3)
    } finally { wrapper.unmount() }
  })

  it('卸载市场插件后原卡片变为未安装，数量不重复也不丢失', async () => {
    const { wrapper, client } = await mountCenter([entry('demo.alpha', '1.0.0')], [installed('demo.alpha', '1.0.0')])
    try {
      await wrapper.findAll('button').find(button => button.text() === '卸载').trigger('click')
      await flushPromises()
      expect(client.uninstallExtension).toHaveBeenCalledTimes(1)
      expect(wrapper.findAll('.plugin-card')).toHaveLength(1)
      expect(wrapper.get('.plugin-card').text()).toContain('未安装')
      expect(wrapper.get('.plugin-card button').text()).toBe('安装')
    } finally { wrapper.unmount() }
  })

  it('管理状态读取失败时不把市场条目误报为未安装，也不提供安装操作', async () => {
    const { wrapper, client } = await mountCenter([entry('demo.alpha', '1.0.0')], [])
    try {
      client.getExtensionManagement.mockRejectedValueOnce(new Error('管理状态读取失败'))
      await wrapper.find('.plugin-toolbar button').trigger('click')
      await flushPromises()
      expect(wrapper.get('.plugin-card').text()).toContain('状态未确认')
      expect(wrapper.get('.plugin-card').text()).not.toContain('未安装')
      expect(wrapper.get('.plugin-card button').attributes('disabled')).toBeDefined()
    } finally { wrapper.unmount() }
  })

  it('更新快照与卸载 204 分别展示运行态和数据结果', () => {
    const update = describeExtensionMutation('update', {
      id: 'demo.update',
      name: '更新插件',
      version: '2.0.0',
      active_version: '2.0.0',
      state: 'running',
    })
    expect(update.operation.text).toContain('已更新到 v2.0.0')
    expect(update.detail.text).toContain('运行中')

    const uninstall = describeExtensionMutation('uninstall', undefined, {
      plugin: installed('demo.remove', '1.0.0', { name: '待卸载插件' }),
      deleteData: false,
    })
    expect(uninstall.operation.text).toContain('待卸载插件 已卸载')
    expect(uninstall.detail).toContain('用户数据已保留')
    expect(uninstall.snapshot).toBeNull()
  })
})
