import type { PanelContribution, PanelRegistry, RegisteredPanel } from './registry'
import { CONSOLE_RIGHT_LOCATION } from './registry'
import type { PluginRuntimeLifecycle } from './lifecycle'

export const KEYMAP_EXTENSION_ID = 'gamer.keymap'
export const KEYMAP_PANEL_ID = 'keymaps'

export interface KeymapExtensionOptions {
  component: unknown
  context: Record<string, unknown>
  runtime?: { start?: () => unknown; stop?: () => unknown }
}

export interface KeymapExtensionHandle {
  contribution: RegisteredPanel
  start(): Promise<unknown>
  stop(): Promise<unknown>
  uninstall(): Promise<void>
}

/**
 * Adapt the Keymap manifest contribution to the Phase 5 Workspace contract.
 * The panel registry and runtime lifecycle deliberately have separate cleanup
 * paths: closing a tab only unmounts UI, while uninstall stops the runtime and
 * unregisters the contribution.
 */
export function registerKeymapExtension(
  registry: PanelRegistry,
  lifecycle: { runtime: PluginRuntimeLifecycle },
  options: KeymapExtensionOptions,
): KeymapExtensionHandle {
  const contribution: PanelContribution = {
    pluginId: KEYMAP_EXTENSION_ID,
    panelId: KEYMAP_PANEL_ID,
    title: '映射',
    icon: '⌨',
    order: 30,
    location: CONSOLE_RIGHT_LOCATION,
    runtime: 'core',
    aliases: ['keymap'],
    component: options.component,
    panelClass: 'extra-tab',
    getProps: () => ({ context: options.context }),
  }
  const registered = registry.register(contribution)
  lifecycle.runtime.register(KEYMAP_EXTENSION_ID, options.runtime || {})
  let removed = false

  return {
    contribution: registry.get(`${KEYMAP_EXTENSION_ID}:${KEYMAP_PANEL_ID}`)!,
    start: () => lifecycle.runtime.start(KEYMAP_EXTENSION_ID),
    stop: () => lifecycle.runtime.stop(KEYMAP_EXTENSION_ID),
    async uninstall() {
      if (removed) return
      removed = true
      await lifecycle.runtime.stop(KEYMAP_EXTENSION_ID)
      registered()
      lifecycle.runtime.unregister(KEYMAP_EXTENSION_ID)
    },
  }
}
