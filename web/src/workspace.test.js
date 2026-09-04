import { describe, expect, it, vi } from 'vitest'
import { createUiBridge, replyToBridgeRequest, UI_BRIDGE_VERSION } from './workspace/bridge'
import { createPanelRegistry, DEFAULT_PANEL_KEY } from './workspace/registry'
import { createPanelUiLifecycle, createPluginRuntimeLifecycle } from './workspace/lifecycle'
import { registerCoreContributions } from './workspace/core-contributions'
import { createServerUiContributionAdapter } from './workspace/plugin-center/adapter/server-ui'

describe('Frontend Plugin Workspace', () => {
  it('keeps contribution order, supports multiple panels per plugin, aliases, and stable fallback', () => {
    const registry = createPanelRegistry()
    registry.register({ pluginId: 'plugin-a', panelId: 'second', title: '第二页', order: 20, location: 'console.right', runtime: 'iframe', keep_alive: 'none' })
    registry.register({ pluginId: 'plugin-a', panelId: 'first', title: '第一页', order: 10, location: 'console.right', runtime: 'core', aliases: ['legacy-first'] })
    registry.register({ pluginId: 'plugin-b', panelId: 'third', title: '第三页', order: 20, location: 'console.right', runtime: 'declarative' })

    expect(registry.getPanels().map(item => item.key)).toEqual(['plugin-a:first', 'plugin-a:second', 'plugin-b:third'])
    expect(registry.resolve('legacy-first')?.key).toBe('plugin-a:first')
    expect(registry.resolve({ pluginId: 'plugin-a', panelId: 'second' })?.key).toBe('plugin-a:second')
    expect(registry.resolve('plugin-a:second')?.keepAlive).toBe('none')
    expect(registry.defaultPanel()?.key).toBe('plugin-a:first')
    expect(registry.unregisterPlugin('plugin-a')).toBe(2)
    expect(registry.defaultPanel()?.key).toBe('plugin-b:third')
    expect(registry.resolve(DEFAULT_PANEL_KEY)).toBeNull()
  })

  it('routes bridge calls through injected host capabilities and scopes storage by plugin', async () => {
    const toast = vi.fn()
    const showOverlay = vi.fn().mockReturnValue('overlay-1')
    const bridge = createUiBridge({
      getContext: () => ({ deviceId: 'dev-a' }),
      toast,
      showOverlay,
      openPanel: vi.fn().mockReturnValue('plugin-a:panel'),
    })
    const meta = { pluginId: 'plugin-a', panelId: 'panel' }

    expect(await bridge.dispatch('context.get', undefined, meta)).toEqual({ deviceId: 'dev-a' })
    expect(await bridge.dispatch('toast.show', { message: 'hello', type: 'success' }, meta)).toBeUndefined()
    expect(toast).toHaveBeenCalledWith('hello', 'success')
    expect(await bridge.dispatch('overlay.show', { id: 'overlay-1' }, meta)).toBe('overlay-1')
    expect(showOverlay).toHaveBeenCalledWith({ id: 'overlay-1' }, meta)
    await bridge.dispatch('storage.set', { key: 'answer', value: 42 }, meta)
    expect(await bridge.dispatch('storage.get', { key: 'answer' }, meta)).toBe(42)
    expect(await bridge.dispatch('storage.get', { key: 'answer' }, { pluginId: 'plugin-b', panelId: 'other' })).toBeUndefined()
    await expect(bridge.dispatch('window.fetch', {}, meta)).rejects.toMatchObject({ code: 'method_not_allowed' })
  })

  it('serializes successful and failed MessageChannel responses with a version', async () => {
    const sent = []
    const bridge = createUiBridge({ getContext: () => ({ ok: true }) })
    await replyToBridgeRequest({ id: '1', method: 'context.get' }, bridge, { postMessage: value => sent.push(value) }, { pluginId: 'p', panelId: 'x' })
    await replyToBridgeRequest({ id: '2', method: 'unknown' }, bridge, { postMessage: value => sent.push(value) }, { pluginId: 'p', panelId: 'x' })
    expect(sent[0]).toMatchObject({ type: 'gamer-ui:response', version: UI_BRIDGE_VERSION, id: '1', ok: true })
    expect(sent[1]).toMatchObject({ type: 'gamer-ui:response', version: UI_BRIDGE_VERSION, id: '2', ok: false, error: { code: 'method_not_allowed' } })
  })

  it('keeps UI close independent from a registered plugin runtime', async () => {
    const ui = createPanelUiLifecycle()
    const start = vi.fn()
    const stop = vi.fn()
    const runtime = createPluginRuntimeLifecycle()
    runtime.register('plugin-a', { start, stop })
    await runtime.start('plugin-a')
    ui.open('plugin-a:panel')
    ui.close('plugin-a:panel')
    expect(runtime.state('plugin-a')).toBe('running')
    expect(stop).not.toHaveBeenCalled()
    await runtime.stop('plugin-a')
    expect(runtime.state('plugin-a')).toBe('stopped')
  })

  it('registers backend UI contributions and cleans stale panels on refresh/unmount', async () => {
    const registry = createPanelRegistry()
    const load = vi.fn()
      .mockResolvedValueOnce({ ui_contributions: [{
        plugin_id: 'plugin-a', panel_id: 'old', title: '旧面板',
        runtime: 'iframe', location: 'console.right', entry: 'ui/old.html',
      }] })
      .mockResolvedValueOnce({ ui_contributions: [{
        plugin_id: 'plugin-a', panel_id: 'new', title: '新面板',
        runtime: 'declarative', location: 'console.right',
      }] })
    const adapter = createServerUiContributionAdapter(registry, { load })

    await adapter.refresh()
    expect(registry.resolve('plugin-a:old')?.iframe?.src).toBe('/api/extensions/plugin-a/ui/old.html')
    await adapter.refresh()
    expect(registry.resolve('plugin-a:old')).toBeNull()
    expect(registry.resolve('plugin-a:new')).not.toBeNull()

    load.mockRejectedValueOnce(new Error('temporary backend failure'))
    await expect(adapter.refresh()).rejects.toThrow('temporary backend failure')
    expect(registry.resolve('plugin-a:new')).not.toBeNull()
    adapter.dispose()
    expect(registry.resolve('plugin-a:new')).toBeNull()
  })

  it('mounts runtime=core contributions through the host component registry and follows panel lifecycle', async () => {
    const registry = createPanelRegistry()
    const load = vi.fn().mockResolvedValue({ ui_contributions: [
      {
        plugin_id: 'gamer.yaml', panel_id: 'automation', title: '自动化',
        runtime: 'core', location: 'console.right', component: 'console.scripts',
        requires_device: true, preferred_width: 440,
      },
      {
        plugin_id: 'gamer.keymap', panel_id: 'keymaps', title: '映射',
        runtime: 'core', location: 'console.right', component: 'console.keymaps',
      },
    ] })
    const adapter = createServerUiContributionAdapter(registry, { load })

    await adapter.refresh()
    // 组件键解析为宿主组件；旧 hash 别名（script/keymap）继续可用；编辑器面板 session keep-alive
    const automation = registry.resolve('gamer.yaml:automation')
    expect(automation?.component).toBeTruthy()
    expect(automation?.keepAlive).toBe('session')
    expect(registry.resolve('script')?.key).toBe('gamer.yaml:automation')
    expect(registry.resolve('keymap')?.key).toBe('gamer.keymap:keymaps')
    // getProps 从 workspace core context 提取对应上下文
    expect(automation?.getProps?.({ scriptRunner: { kind: 'runner' } })).toEqual({ context: { kind: 'runner' } })

    // 扩展停用 → 服务端贡献消失 → 面板从注册表移除（生命周期跟随）
    load.mockResolvedValueOnce({ ui_contributions: [] })
    await adapter.refresh()
    expect(registry.resolve('gamer.yaml:automation')).toBeNull()
    expect(registry.resolve('gamer.keymap:keymaps')).toBeNull()
    adapter.dispose()
  })

  it('renders a placeholder for unrecognized core component keys instead of dropping the panel', async () => {
    const registry = createPanelRegistry()
    const adapter = createServerUiContributionAdapter(registry, {
      load: vi.fn().mockResolvedValue({ ui_contributions: [{
        plugin_id: 'com.third.party', panel_id: 'widget', title: '组件面板',
        runtime: 'core', location: 'console.right', component: 'future.widget',
      }] }),
    })

    await adapter.refresh()
    const panel = registry.resolve('com.third.party:widget')
    expect(panel?.component).toBeTruthy()
    // 占位面板不依赖扩展知识：只标记不可用，不抛错
    expect(String(panel?.title)).toBe('组件面板')
    adapter.dispose()
    expect(registry.resolve('com.third.party:widget')).toBeNull()
  })

  it('bare core contributions (tasks/logs/settings) register through the same contract with gamer.core:tasks as default', () => {
    const registry = createPanelRegistry()
    registerCoreContributions(registry, { activePkg: { value: 'com.demo' } })

    expect(registry.getPanels().map(panel => panel.key)).toEqual([
      'gamer.core:tasks', 'gamer.core:logs', 'gamer.core:settings',
    ])
    expect(registry.defaultPanel()?.key).toBe(DEFAULT_PANEL_KEY)
    expect(registry.resolve('tasks')?.key).toBe('gamer.core:tasks')
    expect(registry.get('gamer.core:tasks')?.getProps?.({})).toEqual({ activePkg: 'com.demo' })
  })
})

