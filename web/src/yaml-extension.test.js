import { describe, expect, it, vi } from 'vitest'
import { createPanelRegistry } from './workspace/registry'
import { createWorkspaceLifecycle } from './workspace/lifecycle'
import {
  registerYamlExtensionPanels,
  YAML_AUTOMATION_PANEL_ID,
  YAML_EXTENSION_ID,
  YAML_FUNCTIONS_PANEL_ID,
} from './workspace/yaml-extension'

describe('YAML vNext panel contribution', () => {
  it('registers automation/functions and removes both panels and runtime on uninstall', async () => {
    const registry = createPanelRegistry()
    const lifecycle = createWorkspaceLifecycle()
    const start = vi.fn()
    const stop = vi.fn()
    const installed = registerYamlExtensionPanels(registry, lifecycle, {
      runtime: { start, stop },
      resolveEntry: (_, entry) => `/fixture/${entry}`,
      aliases: panelId => [`legacy:${panelId}`],
    })

    expect(installed.pluginId).toBe(YAML_EXTENSION_ID)
    expect(installed.panels.map(panel => panel.panelId)).toEqual([
      YAML_AUTOMATION_PANEL_ID,
      YAML_FUNCTIONS_PANEL_ID,
    ])
    expect(registry.resolve('legacy:automation')?.key).toBe(
      `${YAML_EXTENSION_ID}:${YAML_AUTOMATION_PANEL_ID}`,
    )
    expect(
      registry.get({ pluginId: YAML_EXTENSION_ID, panelId: YAML_FUNCTIONS_PANEL_ID })?.iframe?.src,
    ).toBe('/fixture/ui/functions.html')

    await installed.start()
    expect(start).toHaveBeenCalledOnce()
    await installed.uninstall()
    expect(stop).toHaveBeenCalledOnce()
    expect(registry.getPanels().filter(panel => panel.pluginId === YAML_EXTENSION_ID)).toHaveLength(0)
    expect(lifecycle.runtime.state(YAML_EXTENSION_ID)).toBe('missing')
  })
})
