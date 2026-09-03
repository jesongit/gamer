import { describe, expect, it, vi } from 'vitest'
import { createPanelRegistry } from './workspace/registry'
import { createPluginRuntimeLifecycle, createPanelUiLifecycle } from './workspace/lifecycle'
import { createContributionManager, manifestPanels } from './workspace/contribution-manager'

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
  it('maps manifest panels and keeps iframe panels session-alive', () => {
    const panels = manifestPanels(manifest, { resolveEntry: (_, entry) => `/ext/${entry}` })
    expect(panels).toMatchObject([
      { pluginId: 'gamer.keymap', panelId: 'keymaps', runtime: 'iframe', keepAlive: 'session', iframe: { src: '/ext/ui/index.html' } },
      { pluginId: 'gamer.keymap', panelId: 'help', runtime: 'declarative', keepAlive: 'none' },
    ])
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
