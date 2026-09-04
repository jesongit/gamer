import LogsPanel from '../components/LogsPanel.vue'
import SystemPanel from '../components/SystemPanel.vue'
import TaskBoard from '../components/TaskBoard.vue'
import type { PanelContribution, PanelRegistry } from './registry'
import { CONSOLE_RIGHT_LOCATION } from './registry'

type CoreContexts = {
  activePkg?: { value?: string } | string
}

/**
 * Core 自有 UI（ADR-11：任务/日志/设置）。业务面板（自动化/函数/模板/映射）
 * 全部由扩展 manifest 驱动（runtime = "core" + component 键），不再在 Core 壳
 * 里无条件注册；裸 Core 右侧只有 任务|日志|设置。
 */
export function registerCoreContributions(
  registry: PanelRegistry,
  contexts: CoreContexts = {},
) {
  const entries: PanelContribution[] = [
    {
      pluginId: 'gamer.core', panelId: 'tasks', title: '任务', order: 40,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['tasks'],
      component: TaskBoard, panelClass: 'extra-tab',
      getProps: () => ({
        activePkg: contexts.activePkg && typeof contexts.activePkg === 'object' && 'value' in contexts.activePkg
          ? contexts.activePkg.value ?? null
          : contexts.activePkg || null,
      }),
    },
    {
      pluginId: 'gamer.core', panelId: 'logs', title: '日志', order: 50,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['logs'],
      component: LogsPanel, panelClass: 'extra-tab',
    },
    {
      pluginId: 'gamer.core', panelId: 'settings', title: '设置', order: 60,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['settings'],
      component: SystemPanel, panelClass: 'extra-tab',
    },
  ]
  return entries.map(contribution => ({ contribution, unregister: registry.register(contribution) }))
}
