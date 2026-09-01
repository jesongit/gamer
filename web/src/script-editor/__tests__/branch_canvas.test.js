// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick, reactive } from 'vue'
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

const DEEP_NESTED_YAML = [
  'steps:',
  '  - if: true',
  '    then:',
  '    - loop:',
  '        times: 2',
  '        steps:',
  '        - if: true',
  '          then:',
  '          - loop:',
  '              times: 2',
  '              steps:',
  '              - log: deep',
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
    expect(w.emitted('add-here')[0][0]).toEqual(['steps', 0, 'else'])
    expect(w.emitted('add-here')[0][1]).toBe(w.find('button.add').element)
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
    await wrapper.find('button[data-kind="str_app"]').trigger('click') // 启动应用
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
    await wrapper.find('button[data-kind="log"]').trigger('click')
    expect(model.steps[1].then.at(-1).kind).toBe('log')
    expect(model.steps[1].then.at(-1).message.lit).toBe('')
  })
})

describe('StepCanvas：添加下拉与视图保持', () => {
  it('添加步骤为紧凑下拉态：保留插入位置提示，可关闭', async () => {
    const { wrapper } = mountCanvas({ yaml: 'steps:\n  - log: a\n' })
    await wrapper.find('button.add-btn').trigger('click')
    expect(wrapper.find('.add-dropdown-wrap').exists()).toBe(true)
    expect(wrapper.find('.add-overlay').exists()).toBe(false)
    expect(wrapper.find('.add-dialog').exists()).toBe(false)
    expect(wrapper.find('.panel-target').text()).toBe('插入到：主流程 / 末尾')
    await wrapper.find('.add-step-panel button[title="关闭"]').trigger('click')
    expect(wrapper.find('.add-dropdown-wrap').exists()).toBe(false)
  })

  it('嵌套分支「+ 添加」不切换视图：主流程卡仍可见，插入落点在该分支', async () => {
    const { wrapper, model } = mountCanvas()
    await wrapper.find(`[data-step-uuid="${model.steps[0].uuid}"]`).trigger('click')
    await expandCard(wrapper, model.steps[1].uuid) // 展开 if
    const thenContainer = wrapper.find(`[data-step-uuid="${model.steps[1].uuid}"] .card-body .branch-container`)
    await thenContainer.find('button.add').trigger('click')
    expect(wrapper.find('.add-dropdown-wrap').exists()).toBe(true)
    expect(wrapper.find('.panel-target').text()).toContain('如果为真')
    expect(wrapper.text()).toContain('记录日志 top1') // 视图未切进子流程
    await wrapper.find('button[data-kind="log"]').trigger('click')
    expect(model.steps[1].then.at(-1).kind).toBe('log')
    expect(wrapper.find('.add-dropdown-wrap').exists()).toBe(false)
    expect(wrapper.text()).toContain('记录日志 top1') // 插入后仍停在主流程视图
  })

  it('添加菜单靠近屏幕底部时自动翻到锚点上方', async () => {
    const { wrapper } = mountCanvas({ yaml: 'steps:\n  - log: a\n' })
    const originalHeight = window.innerHeight
    const addButton = wrapper.find('button.add-btn').element
    addButton.getBoundingClientRect = () => ({
      left: 100, right: 160, top: 700, bottom: 724, width: 60, height: 24,
      x: 100, y: 700, toJSON: () => ({}),
    })
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 800 })

    await wrapper.find('button.add-btn').trigger('click')
    const panel = wrapper.find('.add-step-panel').element
    panel.getBoundingClientRect = () => ({
      left: 100, right: 320, top: 724, bottom: 1024, width: 220, height: 300,
      x: 100, y: 724, toJSON: () => ({}),
    })
    window.dispatchEvent(new Event('resize'))
    await nextTick()

    expect(wrapper.find('.add-step-panel').attributes('style')).toContain('top: 396px')
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: originalHeight })
  })

  it('非根视图出「返回主流程」按钮，点击回根视图', async () => {
    const { wrapper, model } = mountCanvas()
    expect(wrapper.find('button.back-btn').exists()).toBe(false)
    await expandCard(wrapper, model.steps[1].uuid)
    await expandCard(wrapper, model.steps[1].then[0].uuid)
    await wrapper.findAll('button.focus-btn')[0].trigger('click')
    expect(wrapper.find('button.back-btn').exists()).toBe(true)
    await wrapper.find('button.back-btn').trigger('click')
    expect(wrapper.find('button.back-btn').exists()).toBe(false)
    expect(wrapper.text()).toContain('记录日志 top1')
  })

  it('连续进入专注视图后，返回按进入顺序回到上一个视图', async () => {
    const { wrapper, model } = mountCanvas({ yaml: DEEP_NESTED_YAML })
    const rootIf = model.steps[0]
    const rootLoop = rootIf.then[0]
    const innerIf = rootLoop.steps[0]
    const innerLoop = innerIf.then[0]

    await expandCard(wrapper, rootIf.uuid)
    await expandCard(wrapper, rootLoop.uuid)
    await wrapper.find('button.focus-btn').trigger('click') // rootLoop.steps
    expect(wrapper.findAll('.crumb').map((c) => c.text())).toEqual(['主流程', '如果为真', '循环体'])

    await expandCard(wrapper, innerIf.uuid)
    await expandCard(wrapper, innerLoop.uuid)
    await wrapper.find('button.focus-btn').trigger('click') // innerLoop.steps
    expect(wrapper.findAll('.crumb').map((c) => c.text())).toEqual(['主流程', '如果为真', '循环体', '如果为真', '循环体'])

    await wrapper.find('button.back-btn').trigger('click')
    expect(wrapper.findAll('.crumb').map((c) => c.text())).toEqual(['主流程', '如果为真', '循环体'])
    await wrapper.find('button.back-btn').trigger('click')
    expect(wrapper.find('button.back-btn').exists()).toBe(false)
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
    // 「重命名」：下拉原地变输入框 + 确认按钮；新名字 = 重命名（画布跟随），Esc 取消
    await wrapper.find('button.fn-rename').trigger('click')
    expect(wrapper.find('select.fn-select').exists()).toBe(false)
    expect(wrapper.find('button.fn-rename').text()).toBe('确认') // 确认态按钮文字
    const renameInput = wrapper.find('input[aria-label="函数新名字"]')
    expect(renameInput.element.value).toBe('other')
    await renameInput.setValue('other2')
    await wrapper.find('button.fn-rename').trigger('click') // 确认
    expect(created.model.functions.map((f) => f.name)).toEqual(['login', 'other2'])
    expect(wrapper.find('select.fn-select').element.value).toBe('other2')
    expect(wrapper.find('button.fn-rename').text()).toBe('重命名') // 确认后回到下拉 + 重命名态
    // 添加步骤插入当前函数末尾
    await wrapper.find('button.add-btn').trigger('click')
    await wrapper.find('button[data-kind="str_app"]').trigger('click')
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

  it('initialFn 挂载即直达指定函数；lockFn 时函数名静态展示（不渲染切换下拉）', async () => {
    const created = setupFunctions('login:\n  steps:\n    - log: hi\n\nother:\n  steps:\n    - log: y\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, context: 'function', initialFn: 'other', lockFn: true },
    })
    // 锁定态：无切换下拉，函数名静态展示且画布直接落在指定函数体
    expect(wrapper.find('select.fn-select').exists()).toBe(false)
    const stat = wrapper.find('.fn-static')
    expect(stat.exists()).toBe(true)
    expect(stat.text()).toBe('other')
    expect(wrapper.text()).toContain('记录日志 y')
    // 指定函数本身就是根视图，不应显示返回按钮，更不能返回到默认的第一个函数。
    expect(wrapper.find('button.back-btn').exists()).toBe(false)
    // 重命名仍可用（锁定只去掉切换下拉）：原地变输入框
    await wrapper.find('button.fn-rename').trigger('click')
    expect(wrapper.find('.fn-static').exists()).toBe(false)
    expect(wrapper.find('input[aria-label="函数新名字"]').exists()).toBe(true)
  })

  it('initialFn 不在函数清单时回退默认（第一个函数），保留切换下拉', () => {
    const created = setupFunctions('login:\n  steps:\n    - log: hi\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, context: 'function', initialFn: 'nope' },
    })
    expect(wrapper.text()).toContain('记录日志 hi')
    expect(wrapper.find('select.fn-select').element.value).toBe('login')
  })

  it('reactive 模型下编辑 + undo/redo 正常（组件层接线形态）', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(StepCanvas, { props: { model: created.model, stack: created.stack } })
    await wrapper.find('button.add-btn').trigger('click')
    await wrapper.find('button[data-kind="str_app"]').trigger('click')
    expect(created.model.steps).toHaveLength(2)
    created.stack.undo()
    expect(created.model.steps).toHaveLength(1)
    created.stack.redo()
    expect(created.model.steps).toHaveLength(2)
  })
})
