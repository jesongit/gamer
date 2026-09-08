// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { defaultAnchor } from '../selection'
import AddStepPanel from '../components/AddStepPanel.vue'
import { SE_TARGET_OPTIONS } from '../targets'
import { setupScript } from './component_helpers'

/**
 * AddStepPanel（V1）：流程组（if/repeat/return）+ 函数目录两组（插件/配置包，
 * 经 provide(SE_TARGET_OPTIONS) 注入），选择后经工厂 + CommandStack 插入锚点。
 */

const YAML = 'run:\n  - log: a\n  - log: b\n  - log: c\n'

function mountPanel({ anchor = null, targets = [] } = {}) {
  const created = setupScript(YAML)
  const resolvedAnchor = anchor ?? { containerPath: ['run'], index: created.model.run.length }
  const wrapper = mount(AddStepPanel, {
    props: { stack: created.stack, anchor: resolvedAnchor },
    global: {
      provide: { [SE_TARGET_OPTIONS]: { targets, resolveParams: async () => null } },
    },
  })
  return { ...created, wrapper }
}

const NATIVE_TARGETS = [
  { target: 'tap', group: 'plugin', hint: '点击相对坐标' },
  { target: 'wait_find', group: 'plugin', hint: '等待模板出现' },
  { target: 'login', group: 'package' },
]

describe('AddStepPanel：分组菜单', () => {
  it('流程组恒显三项（if/repeat/return）', () => {
    const { wrapper } = mountPanel()
    const labels = wrapper.findAll('.step-group-label').map((g) => g.text())
    expect(labels).toContain('流程')
    const controlItems = wrapper.findAll('.step-group')[0].findAll('.step-menu-item')
    expect(controlItems.map((b) => b.text())).toEqual(['条件分支', '固定循环', '返回值'])
  })

  it('函数目录分「插件函数 / 配置包函数」两组', () => {
    const { wrapper } = mountPanel({ targets: NATIVE_TARGETS })
    const labels = wrapper.findAll('.step-group-label').map((g) => g.text())
    expect(labels).toEqual(['流程', '插件函数', '配置包函数'])
    expect(wrapper.text()).toContain('tap')
    expect(wrapper.text()).toContain('login')
  })

  it('搜索框过滤函数名', async () => {
    const { wrapper } = mountPanel({ targets: NATIVE_TARGETS })
    await wrapper.find('input[aria-label="搜索函数"]').setValue('wait')
    const fnItems = wrapper.findAll('.fn-item').map((b) => b.text())
    expect(fnItems).toHaveLength(1)
    expect(fnItems[0]).toContain('wait_find')
  })
})

describe('AddStepPanel：插入位置（经工厂 + CommandStack）', () => {
  it('点击控制流项 → 插入到锚点 + undo 移除', async () => {
    const { wrapper, model, stack } = mountPanel({ anchor: { containerPath: ['run'], index: 1 } })
    await wrapper.find('button[data-kind="if"]').trigger('click')
    expect(model.run.map((s) => s.kind)).toEqual(['call', 'if', 'call', 'call'])
    expect(wrapper.emitted('inserted')[0]).toEqual([model.run[1].uuid])
    stack.undo()
    expect(model.run).toHaveLength(3)
  })

  it('点击函数项 → 插入函数调用', async () => {
    const { wrapper, model } = mountPanel({ anchor: { containerPath: ['run'], index: 0 }, targets: NATIVE_TARGETS })
    await wrapper.find('button[data-kind="call:tap"]').trigger('click')
    expect(model.run[0]).toMatchObject({ kind: 'call', fn: 'tap' })
  })

  it('锚点 = 选中卡之后（defaultAnchor 集成）', async () => {
    const created = setupScript(YAML)
    const anchor = defaultAnchor(created.model, created.model.run[1].uuid)
    expect(anchor).toEqual({ containerPath: ['run'], index: 2 })
    const wrapper = mount(AddStepPanel, {
      props: { stack: created.stack, anchor },
      global: { provide: { [SE_TARGET_OPTIONS]: { targets: [], resolveParams: async () => null } } },
    })
    await wrapper.find('button[data-kind="repeat"]').trigger('click')
    expect(created.model.run[2].kind).toBe('repeat')
  })
})
