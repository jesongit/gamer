import type { PanelContribution, PanelRegistry } from './registry'
import { CONSOLE_RIGHT_LOCATION } from './registry'

type CoreComponents = {
  TemplateCapture: unknown
  ScriptRunner: unknown
  KeymapPanel: unknown
  LogsPanel: unknown
  TaskBoard: unknown
  SystemPanel: unknown
}

type CoreContexts = {
  templateCapture: Record<string, unknown>
  scriptRunner: Record<string, unknown>
  keymap: Record<string, unknown>
  activePkg: { value?: string } | string
}

/** Register built-in panels through the same contract later used by plugins. */
export function registerCoreContributions(
  registry: PanelRegistry,
  components: CoreComponents,
  contexts: CoreContexts,
) {
  const entries: PanelContribution[] = [
    {
      pluginId: 'gamer.yaml', panelId: 'templates', title: '模板', icon: '🖼️', order: 10,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', keepAlive: 'session', aliases: ['tpl'],
      component: components.TemplateCapture, panelClass: 'tpl-tab',
      getProps: () => ({ context: contexts.templateCapture }),
    },
    {
      pluginId: 'gamer.yaml', panelId: 'scripts', title: '脚本', icon: '📜', order: 20,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', keepAlive: 'session', aliases: ['script'],
      component: components.ScriptRunner, panelClass: 'script-tab',
      getProps: () => ({ context: contexts.scriptRunner }),
    },
    {
      pluginId: 'gamer.keymap', panelId: 'keymaps', title: '映射', icon: '⌨', order: 30,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['keymap'],
      component: components.KeymapPanel, panelClass: 'extra-tab',
      getProps: () => ({ context: contexts.keymap }),
    },
    {
      pluginId: 'gamer.core', panelId: 'logs', title: '日志', order: 40,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['logs'],
      component: components.LogsPanel, panelClass: 'extra-tab',
    },
    {
      pluginId: 'gamer.core', panelId: 'tasks', title: '任务', order: 50,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['tasks'],
      component: components.TaskBoard, panelClass: 'extra-tab',
      getProps: () => ({
        activePkg: contexts.activePkg && typeof contexts.activePkg === 'object' && 'value' in contexts.activePkg
          ? contexts.activePkg.value ?? null
          : contexts.activePkg || null,
      }),
    },
    {
      pluginId: 'gamer.core', panelId: 'settings', title: '设置', order: 60,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['settings'],
      component: components.SystemPanel, panelClass: 'extra-tab',
    },
  ]
  return entries.map(contribution => ({ contribution, unregister: registry.register(contribution) }))
}
