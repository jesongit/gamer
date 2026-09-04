// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h } from 'vue'
import KeymapPanel from './components/console/KeymapPanel.vue'
import ScriptRunner from './components/console/ScriptRunner.vue'
import TemplateCapture from './components/console/TemplateCapture.vue'
import {
  CORE_PANEL_COMPONENTS,
  applyFunctionsMode,
  resolveCoreComponent,
} from './workspace/core-component-registry'
import { unknownCorePanel } from './workspace/contribution-manager'

describe('Console core panel component registry', () => {
  it('maps manifest component keys to host console components with context extraction', () => {
    const scripts = resolveCoreComponent(CORE_PANEL_COMPONENTS.scripts)
    expect(scripts?.component).toBe(ScriptRunner)
    expect(scripts?.panelClass).toBe('script-tab')
    expect(scripts?.aliases).toContain('script')
    expect(scripts?.getProps?.({ scriptRunner: { kind: 'runner' } })).toEqual({
      context: { kind: 'runner' },
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

  it('functions panel shares the script runner context and defaults runKind to func on mount', () => {
    const functions = resolveCoreComponent(CORE_PANEL_COMPONENTS.functions)
    expect(functions?.component).not.toBe(ScriptRunner)
    expect(functions?.getProps?.({ scriptRunner: { kind: 'runner' } })).toEqual({
      context: { kind: 'runner' },
    })

    // 挂载副作用抽成纯函数：共享上下文的 runKind 切到 func（幂等）
    const runKind = { value: 'script' }
    applyFunctionsMode({ runKind })
    expect(runKind.value).toBe('func')
    applyFunctionsMode({ runKind })
    expect(runKind.value).toBe('func')
    expect(() => applyFunctionsMode(null)).not.toThrow()
    expect(() => applyFunctionsMode({ runKind: 'not-a-ref' })).not.toThrow()
  })
})
