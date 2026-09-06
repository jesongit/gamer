import LogsPanel from '../components/LogsPanel.vue'
import SystemPanel from '../components/SystemPanel.vue'
import TaskBoard from '../components/TaskBoard.vue'
import type { PanelContribution, PanelRegistry } from './registry'
import { CONSOLE_RIGHT_LOCATION } from './registry'

type CoreContexts = {
  /** 当前 Package id（plan §39：数据上下文由 Core Store 统一管理）。 */
  packageId?: { value?: string | null } | string | null
}

/**
 * Core 自有 UI（ADR-11：任务/日志/设置）。业务面板（自动化/函数/模板/映射）
 * 全部由扩展 manifest 驱动（runtime = "core" + component 键），不在 Core 壳
 * 里无条件注册；裸 Core 主导航 = 任务|日志|市场|插件|设置（市场/插件两态由
 * PluginWorkspace 提供，不经本注册表）。
 */
export function registerCoreContributions(
  registry: PanelRegistry,
  contexts: CoreContexts = {},
) {
  const packageIdValue = (): string | null => {
    const ctx = contexts.packageId
    if (ctx && typeof ctx === 'object' && 'value' in ctx) return ctx.value ?? null
    return (ctx as string | null) || null
  }
  const entries: PanelContribution[] = [
    {
      pluginId: 'gamer.core', panelId: 'tasks', title: '任务', order: 40,
      location: CONSOLE_RIGHT_LOCATION, runtime: 'core', aliases: ['tasks'],
      component: TaskBoard, panelClass: 'extra-tab',
      getProps: () => ({ packageId: packageIdValue() }),
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
