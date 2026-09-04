import { defineComponent, h, onMounted } from 'vue'
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

/** 函数库模式：与 console.scripts 共享 scriptRunner 上下文，挂载时默认切到「函数」。 */
const ScriptRunnerFunctionsMode = defineComponent({
  name: 'ConsoleFunctionsPanel',
  props: { context: { type: Object, required: true } },
  setup(props) {
    onMounted(() => applyFunctionsMode(props.context as Record<string, unknown>))
    return () => h(ScriptRunner, { context: props.context })
  },
})

/** 把共享 scriptRunner 上下文的 runKind 切到函数库（幂等；测试可直接驱动）。 */
export function applyFunctionsMode(context: Record<string, unknown> | null | undefined) {
  const runKind = (context as { runKind?: { value?: unknown } } | null | undefined)?.runKind
  if (runKind && typeof runKind === 'object' && 'value' in runKind) runKind.value = 'func'
}

const DESCRIPTORS: Record<string, CorePanelDescriptor> = {
  [CORE_PANEL_COMPONENTS.scripts]: {
    component: ScriptRunner,
    panelClass: 'script-tab',
    aliases: ['script'],
    getProps: context => ({ context: context.scriptRunner }),
  },
  [CORE_PANEL_COMPONENTS.functions]: {
    component: ScriptRunnerFunctionsMode,
    panelClass: 'script-tab',
    getProps: context => ({ context: context.scriptRunner }),
  },
  [CORE_PANEL_COMPONENTS.templates]: {
    component: TemplateCapture,
    panelClass: 'tpl-tab',
    aliases: ['tpl'],
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
