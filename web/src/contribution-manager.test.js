import { describe, expect, it, vi } from 'vitest'
import { createPanelRegistry } from './workspace/registry'
import { createPluginRuntimeLifecycle, createPanelUiLifecycle } from './workspace/lifecycle'
import { createContributionManager, manifestPanels, registerServerUiContributions } from './workspace/contribution-manager'

const manifest = {
  id: 'gamer.keymap',
  ui: {
    contributions: [
      {
        panel_id: 'keymaps', title: '映射', icon: '⌨', order: 30,
        runtime: 'iframe', entry: 'ui/index.html', requires_device: true,
      },
      { panel_id: 'help', title: '帮助', runtime: 'declarative', order: 31 },
    ],
  },
}

describe('Workspace manifest contribution manager', () => {
  it('registers and disposes server contributions without touching runtime state', () => {
    const registry = createPanelRegistry()
    const registered = registerServerUiContributions(registry, [
      {
        plugin_id: 'gamer.keymap', version: '1.0.0', panel_id: 'remote', title: '远端映射',
        runtime: 'iframe', entry: 'ui/index.html', requires_device: true,
      },
    ], { resolveEntry: (manifest, entry) => `/api/extensions/${manifest.id}/${entry}` })

    expect(registered.panels).toHaveLength(1)
    expect(registry.get('gamer.keymap:remote')).toMatchObject({
      runtime: 'iframe', iframe: { src: '/api/extensions/gamer.keymap/ui/index.html' },
    })
    registered.dispose()
    expect(registry.get('gamer.keymap:remote')).toBeNull()
  })

  it('maps manifest panels and keeps iframe panels session-alive', () => {
    const panels = manifestPanels(manifest, { resolveEntry: (_, entry) => `/ext/${entry}` })
    expect(panels).toMatchObject([
      { pluginId: 'gamer.keymap', panelId: 'keymaps', runtime: 'iframe', keepAlive: 'session', iframe: { src: '/ext/ui/index.html' } },
      { pluginId: 'gamer.keymap', panelId: 'help', runtime: 'declarative', keepAlive: 'none' },
    ])
  })

  it('resolves core contributions through resolveCore and merges descriptor aliases', () => {
    const panels = manifestPanels({
      id: 'gamer.yaml',
      ui: { contributions: [
        {
          panel_id: 'automation', title: '自动化', runtime: 'core',
          location: 'console.right', component: 'console.scripts',
          requires_device: true, preferred_width: 440, order: 25,
        },
        {
          panel_id: 'future', title: '未注册', runtime: 'core',
          location: 'console.right', component: 'future.widget',
        },
      ] },
    }, {
      resolveCore: key => (key === 'console.scripts'
        ? { component: { vue: 'script-runner' }, panelClass: 'script-tab', aliases: ['script'] }
        : null),
    })

    expect(panels).toHaveLength(2)
    expect(panels[0]).toMatchObject({
      pluginId: 'gamer.yaml', panelId: 'automation', runtime: 'core',
      component: { vue: 'script-runner' }, panelClass: 'script-tab',
      keepAlive: 'session', aliases: ['script'], order: 25, requiresDevice: true,
    })
    // 未知组件键 → 占位面板（面板可见、内容不可用），不抛错
    expect(panels[1].component).toBeTruthy()
    expect(panels[1].keepAlive).toBe('session')
  })

  it('core contributions without a component key are rejected', () => {
    expect(() => manifestPanels({
      id: 'gamer.yaml',
      ui: { contributions: [
        { panel_id: 'automation', title: '自动化', runtime: 'core', location: 'console.right' },
      ] },
    })).toThrow(/core panel requires component/)
  })

  it('starts runtime independently of opening and closing its panel', async () => {
    const registry = createPanelRegistry()
    const ui = createPanelUiLifecycle()
    const lifecycle = { ui, runtime: createPluginRuntimeLifecycle() }
    const start = vi.fn()
    const stop = vi.fn()
    const manager = createContributionManager(registry, lifecycle)
    const handle = manager.install(manifest, { runtime: { start, stop } })

    await handle.start()
    ui.open(handle.panels[0].key)
    ui.close(handle.panels[0].key)
    expect(lifecycle.runtime.state('gamer.keymap')).toBe('running')
    expect(stop).not.toHaveBeenCalled()
    await handle.uninstall()
    expect(start).toHaveBeenCalledOnce()
    expect(stop).toHaveBeenCalledOnce()
    expect(registry.getPanels()).toEqual([])
    expect(lifecycle.runtime.state('gamer.keymap')).toBe('missing')
  })

  it('cleans registration before rethrowing a guest stop failure', async () => {
    const registry = createPanelRegistry()
    const lifecycle = { runtime: createPluginRuntimeLifecycle() }
    const failure = new Error('guest stop failed')
    const manager = createContributionManager(registry, lifecycle)
    manager.install(manifest, { runtime: { stop: () => { throw failure } } })

    await expect(manager.uninstall('gamer.keymap')).rejects.toBe(failure)
    expect(registry.getPanels()).toEqual([])
    expect(lifecycle.runtime.state('gamer.keymap')).toBe('missing')
  })
})
