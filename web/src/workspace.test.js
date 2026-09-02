import { describe, expect, it, vi } from 'vitest'
import { createUiBridge, replyToBridgeRequest, UI_BRIDGE_VERSION } from './workspace/bridge'
import { createPanelRegistry, DEFAULT_PANEL_KEY } from './workspace/registry'
import { createPanelUiLifecycle, createPluginRuntimeLifecycle } from './workspace/lifecycle'

describe('Frontend Plugin Workspace', () => {
  it('keeps contribution order, supports multiple panels per plugin, aliases, and stable fallback', () => {
    const registry = createPanelRegistry()
    registry.register({ pluginId: 'plugin-a', panelId: 'second', title: '第二页', order: 20, location: 'console.right', runtime: 'iframe' })
    registry.register({ pluginId: 'plugin-a', panelId: 'first', title: '第一页', order: 10, location: 'console.right', runtime: 'core', aliases: ['legacy-first'] })
    registry.register({ pluginId: 'plugin-b', panelId: 'third', title: '第三页', order: 20, location: 'console.right', runtime: 'declarative' })

    expect(registry.getPanels().map(item => item.key)).toEqual(['plugin-a:first', 'plugin-a:second', 'plugin-b:third'])
    expect(registry.resolve('legacy-first')?.key).toBe('plugin-a:first')
    expect(registry.resolve({ pluginId: 'plugin-a', panelId: 'second' })?.key).toBe('plugin-a:second')
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
})
