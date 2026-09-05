// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'
import KeymapPanel from './components/console/KeymapPanel.vue'
import ScriptRunner from './components/console/ScriptRunner.vue'
import TemplateCapture from './components/console/TemplateCapture.vue'
import {
  CORE_PANEL_COMPONENTS,
  resolveCoreComponent,
} from './workspace/core-component-registry'
import { unknownCorePanel } from './workspace/contribution-manager'

describe('Console core panel component registry', () => {
  it('maps manifest component keys to host console components with context extraction', () => {
    const scripts = resolveCoreComponent(CORE_PANEL_COMPONENTS.scripts)
    expect(scripts?.component).toBe(ScriptRunner)
    expect(scripts?.panelClass).toBe('script-tab')
    expect(scripts?.aliases).toContain('script')
    expect(scripts?.getProps?.({ scriptRunner: { scripts: { kind: 'script-panel' } } })).toEqual({
      context: { kind: 'script-panel' },
    })

    const templates = resolveCoreComponent('console.templates')
    expect(templates?.component).toBe(TemplateCapture)
    expect(templates?.getProps?.({ templateCapture: { kind: 'capture' } })).toEqual({
      context: { kind: 'capture' },
    })

    const keymaps = resolveCoreComponent('console.keymaps')
    expect(keymaps?.component).toBe(KeymapPanel)
    expect(keymaps?.aliases).toContain('keymap')
    expect(keymaps?.getProps?.({ keymap: { kind: 'keymap' } })).toEqual({
      context: { kind: 'keymap' },
    })
  })

  it('functions panel binds its own runner scope (no shared runKind mutation)', () => {
    const scripts = resolveCoreComponent(CORE_PANEL_COMPONENTS.scripts)
    const functions = resolveCoreComponent(CORE_PANEL_COMPONENTS.functions)
    // 两个面板是同一宿主组件 + 各自作用域上下文；不存在「挂载即改写共享 runKind」的副作用
    expect(functions?.component).toBe(ScriptRunner)
    expect(functions?.getProps?.({ scriptRunner: { functions: { kind: 'func-panel' } } })).toEqual({
      context: { kind: 'func-panel' },
    })
    // scripts 面板上下文与 functions 面板上下文互不读取对方作用域
    expect(scripts?.getProps?.({ scriptRunner: { functions: { kind: 'func-panel' } } }))
      .toEqual({ context: undefined })
  })

  it('returns null for unknown keys; placeholder descriptor never throws', () => {
    expect(resolveCoreComponent('future.widget')).toBeNull()
    expect(resolveCoreComponent('')).toBeNull()
    const placeholder = unknownCorePanel('future.widget')
    expect(placeholder.component).toBeTruthy()
    // 占位组件可真实挂载（无 props 需求）
    const host = defineComponent({ render: () => h(placeholder.component) })
    const app = createApp(host)
    expect(() => app.mount(document.createElement('div'))).not.toThrow()
    app.unmount()
  })
})
