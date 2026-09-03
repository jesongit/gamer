import type { PanelRegistry } from './registry'
import type { PluginRuntimeLifecycle } from './lifecycle'
import { createContributionManager, type InstalledContribution, type ManifestContributionOptions } from './contribution-manager'

/** Stable IDs shared by the server manifest and the console panel router. */
export const YAML_EXTENSION_ID = 'gamer.yaml'
export const YAML_AUTOMATION_PANEL_ID = 'automation'
export const YAML_FUNCTIONS_PANEL_ID = 'functions'

export interface YamlExtensionPanelOptions extends Omit<ManifestContributionOptions, 'aliases'> {
  aliases?: (panelId: string) => string[]
}

/**
 * Register the two optional vNext panels through the same lifecycle used by
 * installed extensions. The server remains the source of the real iframe
 * assets; this helper is also usable by the local development fixture.
 */
export function registerYamlExtensionPanels(
  registry: PanelRegistry,
  lifecycle: { runtime: PluginRuntimeLifecycle },
  options: YamlExtensionPanelOptions = {},
): InstalledContribution {
  const manager = createContributionManager(registry, lifecycle)
  const aliases = options.aliases
  return manager.install({
    id: YAML_EXTENSION_ID,
    ui: {
      contributions: [
        {
          panel_id: YAML_AUTOMATION_PANEL_ID,
          title: '自动化',
          icon: '⚙️',
          order: 25,
          location: 'console.right',
          runtime: 'iframe',
          requires_device: true,
          preferred_width: 440,
          entry: 'ui/automation.html',
        },
        {
          panel_id: YAML_FUNCTIONS_PANEL_ID,
          title: '函数',
          icon: 'ƒ',
          order: 30,
          location: 'console.right',
          runtime: 'iframe',
          requires_device: false,
          preferred_width: 440,
          entry: 'ui/functions.html',
        },
      ],
    },
  }, {
    ...options,
    aliases: item => aliases?.(item.panel_id) || [],
  })
}
