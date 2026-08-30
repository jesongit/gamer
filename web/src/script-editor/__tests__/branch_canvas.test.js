// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { reactive } from 'vue'
import BranchContainer from '../components/BranchContainer.vue'
import StepCanvas from '../components/StepCanvas.vue'
import { expandCard, setupScript, setupFunctions } from './component_helpers'

/**
 * BranchContainer：嵌套渲染 / 一层内嵌与专注分界（depth≥2 只出专注按钮）；
 * StepCanvas：卡片渲染、点击选中、锚点提示、添加入口、专注视图进出、面包屑路径、
 * ErrorSummary 定位联动（滚动 + 高亮 + 逐层展开）。
 */

const NESTED_YAML = [
  'steps:',
  '  - log: top1',
  '  - if: true',
  '    then:',
  '    - loop:',
  '        times: 2',
  '        steps:',
  '        - log: inner',
  '    else:',
  '    - log: no',
].join('\n')

function mountBranch({ yaml = NESTED_YAML, depth = 0, containerPath = ['steps'], basePath = 'steps', label = '主流程', expandedUuids = null } = {}) {
  const created = setupScript(yaml)
  const wrapper = mount(BranchContainer, {
    props: {
      model: created.model,
      stack: created.stack,
      containerPath,
      basePath,
      label,
      depth,
      expandedUuids,
    },
  })
  return { ...created, wrapper }
}

describe('BranchContainer：嵌套与专注分界', () => {
  it('depth 0 渲染卡片（收起态）', () => {
    const { wrapper } = mountBranch()
    expect(wrapper.findAll('.step-card')).toHaveLength(2)
    expect(wrapper.text()).toContain('记录日志 top1')
    expect(wrapper.text()).toContain('如果 true')
  })

  it('depth 1 内嵌分支内卡片', () => {
    const created = setupScript(NESTED_YAML)
    const ifStep = created.model.steps[1]
    const wrapper = mount(BranchContainer, {
      props: {
        model: created.model,
        stack: created.stack,
        containerPath: ['steps', 1, 'then'],
        basePath: 'steps[1].then',
        label: '如果为真',
        depth: 1,
        expandedUuids: new Set([ifStep.uuid]),
      },
    })
    expect(wrapper.text()).toContain('循环 2 次')
  })

  it('depth 2 只显示专注入口，点击 emit focus(containerPath)', async () => {
    const created = setupScript(NESTED_YAML)
    const ifStep = created.model.steps[1]
    const loopStep = ifStep.then[0]
    const wrapper = mount(BranchContainer, {
      props: {
        model: created.model,
        stack: created.stack,
        containerPath: ['steps', 1, 'then'],
        basePath: 'steps[1].then',
        label: '如果为真',
        depth: 2,
        expandedUuids: new Set([ifStep.uuid]),
      },
    })
    expect(wrapper.findAll('.step-card')).toHaveLength(0)
    expect(wrapper.text()).toContain('进入专注编辑')
    await wrapper.find('button.focus-btn').trigger('click')
    expect(wrapper.emitted('focus')[0]).toEqual([['steps', 1, 'then']])
    expect(loopStep.kind).toBe('loop')
  })

  it('空流程占位 + 容器级添加按钮 emit add-here', async () => {
    const { wrapper } = mountBranch({ yaml: 'steps:\n  - if: true\n    then: []\n    else: []' })
    const created = setupScript('steps:\n  - if: true\n    then: []\n    else: []')
    const ifStep = created.model.steps[0]
    const w = mount(BranchContainer, {
      props: {
        model: created.model,
        stack: created.stack,
        containerPath: ['steps', 0, 'else'],
        basePath: 'steps[0].else',
        label: '如果为假',
        depth: 1,
        expandedUuids: new Set([ifStep.uuid]),
      },
    })
    expect(w.text()).toContain('空流程')
    await w.find('button.add').trigger('click')
    expect(w.emitted('add-here')[0]).toEqual([['steps', 0, 'else']])
    expect(wrapper).toBeTruthy()
  })
})

function mountCanvas({ yaml = NESTED_YAML, props = {} } = {}) {
  const created = setupScript(yaml)
  const wrapper = mount(StepCanvas, {
    props: { model: created.model, stack: created.stack, context: 'script', ...props },
  })
  return { ...created, wrapper }
}

describe('StepCanvas：选中/锚点/添加', () => {

  it('点击卡片选中 + 高亮类 + select 事件', async () => {
    const { wrapper, model } = mountCanvas()
    await wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`).trigger('click')
    expect(wrapper.emitted('select')[0]).toEqual([model.steps[0].uuid])
    const card = wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`)
    expect(card.classes()).toContain('selected')
  })

  it('锚点提示：无选中 = 末尾；选中第 1 卡 = 第 1 步之后', async () => {
    const { wrapper, model } = mountCanvas({ yaml: 'steps:\n  - log: a\n  - log: b\n' })
    expect(wrapper.find('.anchor-hint').text()).toBe('下一条将插入：主流程 / 末尾')
    await wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`).trigger('click')
    expect(wrapper.find('.anchor-hint').text()).toBe('下一条将插入：主流程 / 第 1 步之后')
  })

  it('添加入口：选中卡后插入新步骤并自动选中', async () => {
    const { wrapper, model } = mountCanvas({ yaml: 'steps:\n  - log: a\n' })
    await wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`).trigger('click')
    await wrapper.find('button.add-btn').trigger('click')
    expect(wrapper.find('.add-step-panel').exists()).toBe(true)
    await wrapper.findAll('.entry-btn')[0].trigger('click') // 启动应用
    expect(model.steps.map((s) => s.kind)).toEqual(['log', 'str_app'])
    expect(wrapper.emitted('select').at(-1)).toEqual([model.steps[1].uuid])
    expect(wrapper.find('.add-step-panel').exists()).toBe(false)
  })

  it('容器级 + 添加：清空选中并插入该容器末尾', async () => {
    const { wrapper, model } = mountCanvas()
    await wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`).trigger('click') // 选中 top1
    await expandCard(wrapper, model.steps[1].uuid) // 展开 if
    // if 卡内的 then 容器（depth 1）的 + 添加（排除根容器与专注按钮）
    const thenContainer = wrapper.find(`[data-step-uuid="${model.steps[1].uuid}"] .card-body .branch-container`)
    await thenContainer.find('button.add').trigger('click')
    expect(wrapper.find('.add-step-panel').exists()).toBe(true)
    const logEntry = wrapper.findAll('.entry-btn').filter((b) => b.text().includes('记录日志'))[0]
    await logEntry.trigger('click')
    expect(model.steps[1].then.at(-1).kind).toBe('log')
    expect(model.steps[1].then.at(-1).message.lit).toBe('')
  })
})

describe('StepCanvas：专注视图与面包屑', () => {
  it('深层分支专注进出 + 面包屑路径', async () => {
    const { wrapper, model } = mountCanvas()
    await expandCard(wrapper, model.steps[1].uuid) // if 卡
    await expandCard(wrapper, model.steps[1].then[0].uuid) // loop 卡
    // loop 的循环体容器在 depth 2 → 只有专注按钮
    const focusBtn = wrapper.findAll('button.focus-btn')[0]
    expect(focusBtn).toBeTruthy()
    await focusBtn.trigger('click')
    // 面包屑：主流程 / 如果为真 / 循环体
    const crumbs = wrapper.findAll('.crumb').map((c) => c.text())
    expect(crumbs).toEqual(['主流程', '如果为真', '循环体'])
    // 专注视图内可见 inner 卡
    expect(wrapper.text()).toContain('记录日志 inner')
    expect(wrapper.text()).not.toContain('记录日志 top1')
    // 点击面包屑根返回主流程视图
    await wrapper.findAll('.crumb')[0].trigger('click')
    expect(wrapper.text()).toContain('记录日志 top1')
    // 根视图面包屑只剩「主流程」单节点（与视图重复）→ 不渲染
    expect(wrapper.find('.breadcrumb').exists()).toBe(false)
  })

  it('面包屑中段导航', async () => {
    const { wrapper, model } = mountCanvas()
    await expandCard(wrapper, model.steps[1].uuid)
    await expandCard(wrapper, model.steps[1].then[0].uuid)
    await wrapper.findAll('button.focus-btn')[0].trigger('click')
    // 点击中段「如果为真」→ 仍专注 then 容器
    await wrapper.findAll('.crumb')[1].trigger('click')
    const crumbs = wrapper.findAll('.crumb').map((c) => c.text())
    expect(crumbs).toEqual(['主流程', '如果为真'])
    expect(wrapper.text()).toContain('循环 2 次')
  })

  it('选中卡时面包屑来自 selection.breadcrumb', async () => {
    const { wrapper, model } = mountCanvas()
    await expandCard(wrapper, model.steps[1].uuid)
    await expandCard(wrapper, model.steps[1].then[0].uuid)
    await wrapper.find(`[data-step-uuid="${model.steps[1].then[0].uuid}"]`).trigger('click')
    const crumbs = wrapper.findAll('.crumb').map((c) => c.text())
    expect(crumbs).toEqual(['主流程', '如果为真'])
  })
})

describe('StepCanvas：诊断定位联动（showErrorPanel）', () => {
  it('顶层步骤定位：展开 + 选中 + 瞬态高亮', async () => {
    const { wrapper, model } = mountCanvas({
      yaml: 'steps:\n  - log: a\n  - tap: [2, 2]\n',
      props: {
        showErrorPanel: true,
        diagnostics: [{ code: 'step.coord.range', step_path: 'steps[1]', field: 'at', message: '坐标超出 0~1' }],
      },
    })
    const row = wrapper.findAll('.err-row').find((r) => r.text().includes('steps[1]'))
    await row.trigger('click')
    expect(wrapper.emitted('select').at(-1)).toEqual([model.steps[1].uuid])
    const card = wrapper.find(`[data-step-uuid="${model.steps[1].uuid}"]`)
    expect(card.classes()).toContain('card-highlight')
    expect(card.find('.card-body').exists()).toBe(true) // 自动展开
  })

  it('嵌套步骤定位：展开祖先链 + 专注其宿主容器', async () => {
    const { wrapper, model } = mountCanvas({
      props: {
        showErrorPanel: true,
        // loop 循环体内的步骤（嵌套 2 层 → 需专注）
        diagnostics: [{ code: 'step.field.missing', step_path: 'steps[1].then[0].steps[0]', field: '', message: 'x' }],
      },
    })
    const row = wrapper.findAll('.err-row').find((r) => r.text().includes('then[0].steps[0]'))
    await row.trigger('click')
    expect(wrapper.emitted('select').at(-1)).toEqual([model.steps[1].then[0].steps[0].uuid])
    // 专注到了循环体容器（嵌套 2 > 1），面包屑路径完整
    const crumbs = wrapper.findAll('.crumb').map((c) => c.text())
    expect(crumbs).toEqual(['主流程', '如果为真', '循环体'])
    expect(wrapper.text()).toContain('记录日志 inner')
  })

  it('非步骤路径（params/config）不炸', async () => {
    const { wrapper } = mountCanvas({
      props: {
        showErrorPanel: true,
        diagnostics: [{ code: 'param.decl.format', step_path: 'params[0]', field: 'name', message: 'x' }],
      },
    })
    const row = wrapper.findAll('.err-row')[0]
    await row.trigger('click')
    expect(wrapper.find('.se-canvas').exists()).toBe(true)
  })
})

describe('StepCanvas：函数库上下文', () => {
  it('函数下拉（列出全部函数切换编辑）+ ✏️ 改名 + 面包屑函数名', async () => {
    const created = setupFunctions('login:\n  steps:\n    - log: hi\n\nother:\n  steps:\n    - log: y\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, context: 'function' },
    })
    const nameSel = wrapper.find('select.fn-select')
    expect(nameSel.exists()).toBe(true)
    expect(nameSel.element.value).toBe('login')
    // 列出文件内全部函数（原生 datalist 的按值过滤问题不存在）
    expect(nameSel.findAll('option').map((o) => o.element.value)).toEqual(['login', 'other'])
    expect(wrapper.text()).toContain('记录日志 hi')
    // 函数根视图：面包屑只有函数名一个节点（与函数下拉重复）→ 不渲染
    expect(wrapper.find('.breadcrumb').exists()).toBe(false)
    // 下拉选择已有函数 = 切换（不改名）
    await nameSel.setValue('other')
    expect(wrapper.text()).toContain('记录日志 y')
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'other'])
    // ✏️ 输入新名字 = 重命名当前函数（画布跟随）
    const promptSpy = vi.spyOn(window, 'prompt').mockReturnValue('other2')
    await wrapper.find('button.fn-rename').trigger('click')
    promptSpy.mockRestore()
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'other2'])
    expect(wrapper.find('select.fn-select').element.value).toBe('other2')
    // 添加步骤插入当前函数末尾
    await wrapper.find('button.add-btn').trigger('click')
    await wrapper.findAll('.entry-btn')[0].trigger('click')
    expect(created.model.functions[1].steps).toHaveLength(2)
    created.stack.undo() // 撤销插入步骤
    created.stack.undo() // 撤销改名
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'other'])
  })

  it('「＋ 函数」新增并切换画布到空函数；删除函数可撤销且仅剩一个时禁用', async () => {
    const created = setupFunctions('login:\n  steps:\n    - log: hi\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, context: 'function' },
    })
    const addFnBtn = wrapper.find('button.fn-add')
    const delFnBtn = () => wrapper.find('button.fn-btn-danger')
    expect(delFnBtn().attributes('disabled')).toBeDefined() // 仅一个函数：删除禁用
    await addFnBtn.trigger('click') // ＋ 函数
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'func1'])
    expect(wrapper.find('select.fn-select').element.value).toBe('func1')
    expect(wrapper.text()).not.toContain('记录日志 hi') // 画布已切到空的 func1
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)
    await delFnBtn().trigger('click')
    expect(created.model.functions.map((f) => f.name)).toEqual(['login'])
    created.stack.undo()
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'func1'])
    confirmSpy.mockRestore()
  })

  it('reactive 模型下编辑 + undo/redo 正常（组件层接线形态）', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(StepCanvas, { props: { model: created.model, stack: created.stack } })
    await wrapper.find('button.add-btn').trigger('click')
    await wrapper.findAll('.entry-btn')[0].trigger('click')
    expect(created.model.steps).toHaveLength(2)
    created.stack.undo()
    expect(created.model.steps).toHaveLength(1)
    created.stack.redo()
    expect(created.model.steps).toHaveLength(2)
  })
})
