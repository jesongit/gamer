import { pluginPanel } from './plugin-module-loader'
import type { CorePanelDescriptor } from './contribution-manager'
export const CORE_PANEL_COMPONENTS = {
  scripts: 'console.scripts', functions: 'console.functions', templates: 'console.templates',
  keymaps: 'console.keymaps', video: 'VideoWorkbench',
} as const
export function resolveCoreComponent(componentKey: string, pluginId = ''): CorePanelDescriptor | null {
  return pluginPanel(pluginId, String(componentKey || '').trim())
}
