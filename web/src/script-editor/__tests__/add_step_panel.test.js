// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { defaultAnchor } from '../selection'
import AddStepPanel from '../components/AddStepPanel.vue'
import { setupScript, setupFunctions } from './component_helpers'

/**
 * AddStepPanel：PANEL_GROUPS 分组下拉/上下文过滤（script 隐藏 return）/
 * 选择后经工厂 + CommandStack 插入到锚点。
 */

const YAML = 'steps:\n  - log: a\n  - log: b\n  - log: c\n'

function mountPanel({ context = 'script', anchor = null } = {}) {
  const created = setupScript(YAML)
  const resolvedAnchor = anchor ?? { containerPath: ['steps'], index: created.model.steps.length }
  const wrapper = mount(AddStepPanel, {
    props: { context, stack: created.stack, anchor: resolvedAnchor },
  })
  return { ...created, wrapper }
}

describe('AddStepPanel：分组下拉', () => {
  it('script 上下文：五个分组（隐藏函数专用）', () => {
    const { wrapper } = mountPanel()
    const labels = wrapper.findAll('optgroup').map((g) => g.attributes('label'))
    expect(labels).toEqual(['应用', '操作', '识别', '流程', '复用'])
  })

  it('script 上下文隐藏 return（18 项）；function 上下文可见', () => {
    const { wrapper } = mountPanel({ context: 'script' })
    expect(wrapper.text()).not.toContain('返回布尔值')
    expect(wrapper.findAll('select[aria-label="选择步骤类型"] option')).toHaveLength(19) // 含占位项
    const fn = setupFunctions('login:\n  steps:\n    - log: x\n')
    const w2 = mount(AddStepPanel, {
      props: { context: 'function', stack: fn.stack, anchor: { containerPath: ['functions', 'login', 'steps'], index: 1 } },
    })
    expect(w2.text()).toContain('返回布尔值')
    expect(w2.findAll('select[aria-label="选择步骤类型"] option')).toHaveLength(20) // 含占位项
  })

  it('下拉选择步骤类型后立即插入', async () => {
    const { wrapper, model } = mountPanel()
    await wrapper.find('select[aria-label="选择步骤类型"]').setValue('color')
    expect(model.steps.at(-1).kind).toBe('color')
  })
})

describe('AddStepPanel：插入位置（经工厂 + CommandStack）', () => {
  it('锚点 index=1 → 插到第 2 位 + undo 移除', async () => {
    const { wrapper, model, stack } = mountPanel({ anchor: { containerPath: ['steps'], index: 1 } })
    await wrapper.find('select[aria-label="选择步骤类型"]').setValue('str_app')
    expect(model.steps.map((s) => s.kind)).toEqual(['log', 'str_app', 'log', 'log'])
    expect(wrapper.emitted('inserted')[0]).toEqual([model.steps[1].uuid])
    stack.undo()
    expect(model.steps).toHaveLength(3)
  })

  it('锚点 = 选中卡之后（defaultAnchor 集成）', async () => {
    const created = setupScript(YAML)
    const anchor = defaultAnchor(created.model, created.model.steps[1].uuid)
    expect(anchor).toEqual({ containerPath: ['steps'], index: 2 })
    const wrapper = mount(AddStepPanel, {
      props: { context: 'script', stack: created.stack, anchor },
    })
    await wrapper.find('select[aria-label="选择步骤类型"]').setValue('str_app')
    expect(created.model.steps[2].kind).toBe('str_app')
  })

  it('锚点 = 末尾', async () => {
    const { wrapper, model } = mountPanel()
    await wrapper.find('select[aria-label="选择步骤类型"]').setValue('str_app')
    expect(model.steps[model.steps.length - 1].kind).toBe('str_app')
  })
})
