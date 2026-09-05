import KeymapPanel from '../components/console/KeymapPanel.vue'
import ScriptRunner from '../components/console/ScriptRunner.vue'
import TemplateCapture from '../components/console/TemplateCapture.vue'
import type { CorePanelDescriptor } from './contribution-manager'

/**
 * 宿主组件键 → Console 内置 Vue 组件的解析表。manifest 写 `runtime = "core"`
 * + `component = "<键>"`（服务端只透传不解释），面板出现/消失跟随扩展生命
 * 周期；本表是组件名的唯一前端知识。
 */
export const CORE_PANEL_COMPONENTS = {
  scripts: 'console.scripts',
  functions: 'console.functions',
  templates: 'console.templates',
  keymaps: 'console.keymaps',
} as const

const DESCRIPTORS: Record<string, CorePanelDescriptor> = {
  [CORE_PANEL_COMPONENTS.scripts]: {
    component: ScriptRunner,
    panelClass: 'script-tab',
    aliases: ['script'],
    // 脚本/函数是同一运行器机制的两份面板作用域上下文（各自锁定资源类型与
    // 编辑模式，互不串台）；context.scriptRunner 由壳装配为 {scripts, functions}。
    getProps: context => {
      const runner = context.scriptRunner as { scripts?: unknown } | undefined
      return { context: runner?.scripts }
    },
  },
  [CORE_PANEL_COMPONENTS.functions]: {
    component: ScriptRunner,
    panelClass: 'script-tab',
    getProps: context => {
      const runner = context.scriptRunner as { functions?: unknown } | undefined
      return { context: runner?.functions }
    },
  },
  [CORE_PANEL_COMPONENTS.templates]: {
    component: TemplateCapture,
    panelClass: 'tpl-tab',
    getProps: context => ({ context: context.templateCapture }),
  },
  [CORE_PANEL_COMPONENTS.keymaps]: {
    component: KeymapPanel,
    panelClass: 'extra-tab',
    aliases: ['keymap'],
    getProps: context => ({ context: context.keymap }),
  },
}

/** 已知组件键 → 描述符；未知键返回 null（由调用方决定占位行为）。 */
export function resolveCoreComponent(componentKey: string): CorePanelDescriptor | null {
  return DESCRIPTORS[String(componentKey || '').trim()] || null
}
