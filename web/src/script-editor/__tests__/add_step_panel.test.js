// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { defaultAnchor } from '../selection'
import AddStepPanel from '../components/AddStepPanel.vue'
import { setupScript, setupFunctions } from './component_helpers'

/**
 * AddStepPanel：PANEL_GROUPS 分组/搜索过滤/上下文过滤（script 隐藏 return）/
 * 点击后经工厂 + CommandStack 插入到锚点。
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

describe('AddStepPanel：分组与过滤', () => {
  it('script 上下文：五个分组（隐藏函数专用）', () => {
    const { wrapper } = mountPanel()
    const labels = wrapper.findAll('.group-label').map((g) => g.text())
    expect(labels).toEqual(['应用', '操作', '识别', '流程', '复用'])
  })

  it('script 上下文隐藏 return（16 项）；function 上下文可见', () => {
    const { wrapper } = mountPanel({ context: 'script' })
    expect(wrapper.text()).not.toContain('返回布尔值')
    expect(wrapper.findAll('.entry-btn')).toHaveLength(16)
    const fn = setupFunctions('login:\n  steps:\n    - log: x\n')
    const w2 = mount(AddStepPanel, {
      props: { context: 'function', stack: fn.stack, anchor: { containerPath: ['functions', 'login', 'steps'], index: 1 } },
    })
    expect(w2.text()).toContain('返回布尔值')
    expect(w2.findAll('.entry-btn')).toHaveLength(17)
  })

  it('搜索过滤：判断颜色 只剩判断颜色', async () => {
    const { wrapper } = mountPanel()
    await wrapper.find('input[aria-label="搜索步骤类型"]').setValue('判断颜色')
    expect(wrapper.findAll('.entry-btn')).toHaveLength(1)
    expect(wrapper.findAll('.entry-btn')[0].text()).toContain('判断颜色')
  })

  it('搜索无结果提示', async () => {
    const { wrapper } = mountPanel()
    await wrapper.find('input[aria-label="搜索步骤类型"]').setValue('不存在的东西')
    expect(wrapper.text()).toContain('没有匹配')
  })
})

describe('AddStepPanel：插入位置（经工厂 + CommandStack）', () => {
  it('锚点 index=1 → 插到第 2 位 + undo 移除', async () => {
    const { wrapper, model, stack } = mountPanel({ anchor: { containerPath: ['steps'], index: 1 } })
    await wrapper.findAll('.entry-btn')[0].trigger('click') // 启动应用
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
    await wrapper.findAll('.entry-btn')[0].trigger('click')
    expect(created.model.steps[2].kind).toBe('str_app')
  })

  it('锚点 = 末尾', async () => {
    const { wrapper, model } = mountPanel()
    await wrapper.findAll('.entry-btn')[0].trigger('click')
    expect(model.steps[model.steps.length - 1].kind).toBe('str_app')
  })
})
