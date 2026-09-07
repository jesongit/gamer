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

// ---------------------------------------------------------------------------
// Phase 1 市场契约（免签名）：schema_version=2 registry 三插件渲染（含 builtin
// 的 gamer.video）、安装确认弹窗展示 ID/版本/来源/执行形态/宿主要求/权限增量、
// host_feature_unavailable 转「升级 Gamer」提示、官方安装不再上传 proof。
//（复用文件顶部已 import 的 PluginCenter / mount / flushPromises）
// ---------------------------------------------------------------------------

const ZERO_SHA256 = '0'.repeat(64) // 与 crypto.digest stub（全零 32 字节）匹配

const REGISTRY_V2 = {
  schema_version: 2,
  plugins: [
    {
      id: 'gamer.yaml', version: '3.1.1', name: '自动化',
      download_url: '/plugins/gamer.yaml-3.1.1.gplugin', sha256: ZERO_SHA256,
      execution: { kind: 'wasm' }, permissions: ['resource.read'],
    },
    {
      id: 'gamer.keymap', version: '1.0.1', name: '按键映射',
      download_url: '/plugins/gamer.keymap-1.0.1.gplugin', sha256: ZERO_SHA256,
      execution: { kind: 'wasm' },
    },
    {
      id: 'gamer.video', version: '1.0.0', name: '视频工作台',
      download_url: '/plugins/gamer.video-1.0.0.gplugin', sha256: ZERO_SHA256,
      execution: { kind: 'builtin', host_version: '>=1.3.0' },
      permissions: ['media.read', 'media.record'],
    },
  ],
}

function archiveBytesResponse() {
  const bytes = new Uint8Array([80, 75, 3, 4])
  return { ok: true, status: 200, headers: { get: () => null }, arrayBuffer: async () => bytes.buffer }
}

function jsonResponseFor(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: name => (/content-type/i.test(name) ? 'application/json' : null) },
    json: async () => body,
  }
}

async function mountMarketCenter(apiClient) {
  const wrapper = mount(PluginCenter, {
    props: { open: true, apiClient },
    global: { stubs: { Teleport: true } },
  })
  await flushPromises()
  return { wrapper, apiClient }
}

describe('Phase 1 免签名市场（registry v2 + 确认弹窗 + 错误分型）', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('confirm', vi.fn(() => true))
    vi.stubGlobal('crypto', { subtle: { digest: async () => new Uint8Array(32).buffer } })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('renders a schema_version=2 registry: three plugins, video marked builtin with host requirement', async () => {
    fetch.mockImplementation(async url => (
      String(url).includes('registry.json') ? jsonResponseFor(200, REGISTRY_V2) : jsonResponseFor(200, { extensions: [] })
    ))
    const apiClient = { getExtensionManagement: vi.fn().mockResolvedValue({ extensions: [] }) }
    const { wrapper } = await mountMarketCenter(apiClient)

    const cards = wrapper.findAll('.plugin-card')
    expect(cards).toHaveLength(3)
    const videoCard = wrapper.findAll('.plugin-card').find(card => card.text().includes('gamer.video'))
    expect(videoCard).toBeTruthy()
    expect(videoCard.text()).toContain('宿主预置')
    expect(videoCard.text()).toContain('>=1.3.0')
    // v2 registry 无签名字段：界面不出现签名概念
    expect(wrapper.find('.plugin-center-body').text()).not.toContain('签名')
    // 三插件都有固定 sha256 → 安装按钮可用
    for (const button of wrapper.findAll('.plugin-card .btn-primary')) {
      expect(button.attributes('disabled')).toBeUndefined()
    }
  })

  it('install confirm dialog shows id/version/source/execution/host requirement/permission diff and installs without proof', async () => {
    fetch.mockImplementation(async url => {
      if (String(url).includes('registry.json')) return jsonResponseFor(200, REGISTRY_V2)
      return archiveBytesResponse()
    })
    const apiClient = {
      getExtensionManagement: vi.fn().mockResolvedValue({ extensions: [] }),
      inspectExtension: vi.fn().mockResolvedValue({
        id: 'gamer.video', version: '1.0.0', name: '视频工作台',
        execution: { kind: 'builtin', host_version: '>=1.3.0' },
        permissions: ['media.read', 'media.record', 'media.write'],
      }),
      installExtension: vi.fn().mockResolvedValue({ id: 'gamer.video', version: '1.0.0', state: 'installed' }),
      enableExtension: vi.fn().mockResolvedValue({ id: 'gamer.video', state: 'enabled' }),
    }
    const { wrapper } = await mountMarketCenter(apiClient)

    const videoCard = wrapper.findAll('.plugin-card').find(card => card.text().includes('gamer.video'))
    await videoCard.find('button.btn-primary').trigger('click')
    await flushPromises()

    expect(globalThis.confirm).toHaveBeenCalledTimes(1)
    const message = vi.mocked(globalThis.confirm).mock.calls[0][0]
    expect(message).toContain('gamer.video')
    expect(message).toContain('1.0.0')
    expect(message).toContain('官方市场')
    expect(message).toContain('宿主预置')
    expect(message).toContain('>=1.3.0')
    expect(message).toContain('新增权限：media.read、media.record、media.write')
    // 普通用户界面不出现密钥/proof 概念
    expect(message).not.toMatch(/签名|proof|密钥/i)

    // inspect 只带来源记录；proof 头已随签名门禁移除
    expect(apiClient.inspectExtension).toHaveBeenCalledWith(expect.anything(), { source: 'official' })
    expect(apiClient.installExtension).toHaveBeenCalledWith(expect.anything(), { source: 'official', permissionConfirmed: true })
    // 安装即启用，服务端无需重启/刷新
    expect(apiClient.enableExtension).toHaveBeenCalledWith('gamer.video')
    expect(wrapper.find('.plugin-alert.info').text()).toContain('已安装')
  })

  it('host_feature_unavailable install failure tells the user to upgrade Gamer instead of retrying', async () => {
    fetch.mockImplementation(async url => {
      if (String(url).includes('registry.json')) return jsonResponseFor(200, REGISTRY_V2)
      return archiveBytesResponse()
    })
    const apiClient = {
      getExtensionManagement: vi.fn().mockResolvedValue({ extensions: [] }),
      inspectExtension: vi.fn().mockResolvedValue({
        id: 'gamer.video', version: '1.0.0', name: '视频工作台',
        execution: { kind: 'builtin', host_version: '>=1.3.0' },
        permissions: ['media.read'],
      }),
      installExtension: vi.fn().mockRejectedValue(
        Object.assign(new Error('未知 builtin id'), { code: 'host_feature_unavailable', status: 400 }),
      ),
    }
    const { wrapper } = await mountMarketCenter(apiClient)

    const videoCard = wrapper.findAll('.plugin-card').find(card => card.text().includes('gamer.video'))
    await videoCard.find('button.btn-primary').trigger('click')
    await flushPromises()

    const alert = wrapper.find('.plugin-alert.error')
    expect(alert.exists()).toBe(true)
    expect(alert.text()).toContain('升级 Gamer')
    expect(wrapper.emitted('changed')).toBeFalsy()
  })
})
