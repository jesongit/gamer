// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { makeStep } from '../factories'
import { STEP_KINDS } from '../model'
import { stepSummary } from '../components/kinds'
import StepCard from '../components/StepCard.vue'
import { expandCard, setupScript } from './component_helpers'

/**
 * StepCard（19 类）：收起态摘要（§9 文案）、展开态强类型控件经 CommandStack 生效、
 * 字段错误按 Diagnostic.field 标红、选中高亮、上移/下移/复制/删除。
 */

const YAML_BY_KIND = {
  str_app: 'str_app',
  cls_app: 'cls_app',
  tap: 'tap: [0.5, 0.5]',
  swipe: 'swipe:\n      fm: [0.3, 0.5]\n      to: [0.7, 0.5]\n      time: 500ms',
  key: 'key: BACK',
  text: 'text: ""',
  log: 'log: ""',
  wait: 'wait: 1s',
  find: 'find: ""',
  match: 'match:\n    - "":\n      - log: x',
  check: 'check: ""\n    throw: ""',
  color: 'color:\n      at: [0.5, 0.5]\n      expect:\n        - "":\n          - log: x',
  if: 'if: true',
  loop: 'loop:\n      steps:\n        - log: x',
  break: 'break',
  call: 'call: ""',
  func: 'func: ""',
  throw: 'throw: null',
  return: 'return: true',
}

const SUMMARY_BY_KIND = {
  str_app: '启动当前应用',
  cls_app: '关闭当前应用',
  tap: '点击坐标 0.5, 0.5',
  swipe: '从 0.3, 0.5 滑到 0.7, 0.5 · 500ms',
  key: '按键 BACK',
  text: '输入文本',
  log: '记录日志',
  wait: '等待 1s',
  find: '等待并点击 （未选模板）',
  match: '按顺序匹配 1 个模板',
  check: '检查 （未选模板）',
  color: '在 0.5, 0.5 判断 1 种颜色',
  if: '如果 true',
  loop: '无限循环',
  break: '跳出循环',
  call: '调用脚本 （未填目标）',
  func: '调用函数 （未填目标）',
  throw: '终止',
  return: '返回 true',
}

function mountCard({ yaml = 'steps:\n  - log: hello\n', index = 0, props = {} } = {}) {
  const { model, stack } = setupScript(yaml)
  const wrapper = mount(StepCard, {
    props: {
      model,
      stack,
      step: model.steps[index],
      containerPath: ['steps'],
      basePath: 'steps',
      index,
      context: 'script',
      ...props,
    },
  })
  return { wrapper, model, stack }
}

describe('StepCard：收起态摘要（§9 文案，19 类全覆盖）', () => {
  for (const kind of STEP_KINDS) {
    it(`${kind} 摘要`, () => {
      expect(stepSummary(makeStep(kind))).toBe(SUMMARY_BY_KIND[kind])
      const { wrapper } = mountCard({ yaml: `steps:\n  - ${YAML_BY_KIND[kind]}` })
      expect(wrapper.find('.summary').text()).toBe(SUMMARY_BY_KIND[kind])
      expect(wrapper.find('.kind-name').text()).toBeTruthy()
      expect(wrapper.find('.step-no').text()).toBe('#1')
    })
  }
})

describe('StepCard：选中/展开', () => {
  it('点击卡片 → emit select(uuid)', () => {
    const { wrapper, model } = mountCard({})
    wrapper.find('.card-head').trigger('click')
    expect(wrapper.emitted('select')[0]).toEqual([model.steps[0].uuid])
  })

  it('expandedUuids 集合驱动展开；toggle-expand 事件上抛', () => {
    const created = setupScript('steps:\n  - log: hello\n')
    const uuid = created.model.steps[0].uuid
    const wrapper = mount(StepCard, {
      props: {
        model: created.model,
        stack: created.stack,
        step: created.model.steps[0],
        containerPath: ['steps'],
        basePath: 'steps',
        index: 0,
        expandedUuids: new Set([uuid]),
      },
    })
    expect(wrapper.find('.card-body').exists()).toBe(true)
    wrapper.find('button[title="收起"]').trigger('click')
    expect(wrapper.emitted('toggle-expand')[0]).toEqual([uuid])
  })
})

describe('StepCard：展开编辑经 CommandStack 生效', () => {
  it('tap 坐标编辑 + undo', async () => {
    const { wrapper, model, stack } = mountCard({ yaml: 'steps:\n  - tap: [0.5, 0.8]' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="坐标X"]').setValue('0.2')
    expect(model.steps[0].at.lit).toEqual([0.2, 0.8])
    stack.undo()
    expect(model.steps[0].at.lit).toEqual([0.5, 0.8])
  })

  it('key 枚举下拉', async () => {
    const { wrapper, model } = mountCard({ yaml: 'steps:\n  - key: BACK' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('select[aria-label="按键"]').setValue('HOME')
    expect(model.steps[0].key.lit).toBe('HOME')
  })

  it('wait 随机区间开关', async () => {
    const { wrapper, model } = mountCard({ yaml: 'steps:\n  - wait: 1s' })
    await expandCard(wrapper, model.steps[0].uuid)
    const toggle = wrapper.find('input[type="checkbox"]')
    await toggle.setValue(true)
    expect(model.steps[0].duration_max).toEqual({ lit: '1s' })
    await toggle.setValue(false)
    expect(model.steps[0].duration_max).toBeNull()
  })

  it('find：模板/verify/障碍增删', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - find: login.png\n    verify: false',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="主模板"]').setValue('account.png')
    expect(model.steps[0].template.lit).toBe('account.png')
    await wrapper.find('.field-check input').setValue(true)
    expect(model.steps[0].verify).toBe(true)
    await wrapper.find('button[title="添加障碍"]').trigger('click')
    expect(model.steps[0].block).toHaveLength(1)
    await wrapper.find('button[title="删除障碍"]').trigger('click')
    expect(model.steps[0].block).toHaveLength(0)
  })

  it('find/match 超时未勾选展示默认行为提示，勾选后隐藏', async () => {
    const checkboxByText = (wrapper, text) =>
      wrapper.findAll('label.field-check').find((l) => l.text().includes(text)).find('input[type="checkbox"]')
    const find = mountCard({ yaml: 'steps:\n  - find: login.png' })
    await expandCard(find.wrapper, find.model.steps[0].uuid)
    expect(find.wrapper.text()).toContain('默认 30min')
    await checkboxByText(find.wrapper, '等待超时').setValue(true)
    expect(find.wrapper.text()).not.toContain('默认 30min')
    expect(find.model.steps[0].timeout).toEqual({ lit: '30s' })

    const match = mountCard({ yaml: 'steps:\n  - match:\n    - a.png:\n      - log: x' })
    await expandCard(match.wrapper, match.model.steps[0].uuid)
    expect(match.wrapper.text()).toContain('未配置仅检测一轮')
    await checkboxByText(match.wrapper, '轮询超时').setValue(true)
    expect(match.wrapper.text()).not.toContain('未配置仅检测一轮')
    expect(match.model.steps[0].timeout).toEqual({ lit: '30s' })
  })

  it('match：候选增删', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - match:\n    - a.png:\n      - log: x',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('button[title="添加候选"]').trigger('click')
    expect(model.steps[0].candidates).toHaveLength(2)
    await wrapper.find('button[title="删除候选"]').trigger('click')
    expect(model.steps[0].candidates).toHaveLength(1)
  })

  it('match：候选勾选命中点击（经命令栈，可撤销）', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: 'steps:\n  - match:\n    - a.png:\n      - log: x',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="命中点击1"]').setValue(true)
    expect(model.steps[0].candidates[0].click).toBe(true)
    stack.undo()
    expect(model.steps[0].candidates[0].click).toBe(false)
  })

  it('color：hex 输入', async () => {
    const { wrapper, model } = mountCard({
      yaml: "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - '123456':\n          - log: x",
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="颜色1hex"]').setValue('ff8800')
    expect(model.steps[0].expect[0].color.lit).toBe('ff8800')
  })

  it('color：候选勾选命中点击（经命令栈，可撤销）', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: "steps:\n  - color:\n      at: [0.5, 0.5]\n      expect:\n        - '123456':\n          - log: x",
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="命中点击1"]').setValue(true)
    expect(model.steps[0].expect[0].click).toBe(true)
    stack.undo()
    expect(model.steps[0].expect[0].click).toBe(false)
  })

  it('if：字面量 ↔ 布尔参数切换', async () => {
    const created = setupScript("params:\n  - 'bool:enable:是否启用'\nsteps:\n  - if: true")
    const wrapper = mount(StepCard, {
      props: {
        model: created.model,
        stack: created.stack,
        step: created.model.steps[0],
        containerPath: ['steps'],
        basePath: 'steps',
        index: 0,
        params: created.model.params,
      },
    })
    const model = created.model
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.findAll('button.mode-btn')[1].trigger('click')
    expect(model.steps[0].cond).toEqual({ ref: 'enable' })
    await wrapper.findAll('button.mode-btn')[0].trigger('click')
    expect(model.steps[0].cond).toEqual({ lit: true })
  })

  it('loop：次数与 0（无限）', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - loop:\n      times: 3\n      steps:\n        - log: x',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    const times = wrapper.find('input[aria-label="循环次数"]')
    await times.setValue('5')
    await times.trigger('change')
    expect(model.steps[0].times).toBe(5)
    await times.setValue('0')
    await times.trigger('change')
    expect(model.steps[0].times).toBe(0)
  })

  it('call 目标与 args', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - call: sub.yaml\n    args:\n      enable: true',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="目标脚本"]').setValue('other.yaml')
    expect(model.steps[0].target).toBe('other.yaml')
    await wrapper.find('button[title="添加实参"]').trigger('click')
    expect(Object.keys(model.steps[0].args)).toEqual(['enable', 'param1'])
    await wrapper.find('button[title="删除实参"]').trigger('click')
    expect(Object.keys(model.steps[0].args)).toEqual(['param1'])
  })

  it('throw 原因可空', async () => {
    const { wrapper, model } = mountCard({ yaml: 'steps:\n  - throw: 出错了' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="终止原因"]').setValue('')
    expect(model.steps[0].message).toBeNull()
    await wrapper.find('input[aria-label="终止原因"]').setValue('again')
    expect(model.steps[0].message).toBe('again')
  })

  it('str_app 展开只显示裸动作提示', async () => {
    const { wrapper, model } = mountCard({ yaml: 'steps:\n  - str_app' })
    await expandCard(wrapper, model.steps[0].uuid)
    expect(wrapper.find('.card-body').text()).toContain('裸动作')
  })
})

describe('StepCard：错误标红定位', () => {
  it('Diagnostic.field → 控件红框 + 卡片错误徽标', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - tap: [0.5, 0.8]',
      props: {
        diagnostics: [
          { code: 'step.coord.range', step_path: 'steps[0]', field: 'at', message: '坐标超出 0~1' },
          { code: 'step.field.missing', step_path: 'steps[1]', field: 'at', message: '别的卡片的错误' },
        ],
      },
    })
    expect(wrapper.find('.step-card').classes()).toContain('has-error')
    expect(wrapper.find('.err-badge').text()).toBe('1')
    await expandCard(wrapper, model.steps[0].uuid)
    expect(wrapper.find('.cell-editor').classes()).toContain('cell-error')
    expect(wrapper.text()).toContain('坐标超出 0~1')
    expect(wrapper.text()).not.toContain('别的卡片的错误')
  })

  it('非本步骤字段的诊断不标红', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - tap: [0.5, 0.8]',
      props: { diagnostics: [{ code: 'x', step_path: 'steps[0]', field: 'timeout', message: '无关字段' }] },
    })
    expect(wrapper.find('.step-card').classes()).toContain('has-error')
    await expandCard(wrapper, model.steps[0].uuid)
    expect(wrapper.find('.cell-editor').classes()).not.toContain('cell-error')
  })
})

describe('StepCard：上移/下移/复制/删除（经 CommandStack）', () => {
  it('边界禁用 + 下移/上移 + undo', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: 'steps:\n  - log: a\n  - log: b\n  - log: c',
    })
    const first = wrapper.findComponent(StepCard)
    expect(first.find('button[title="上移"]').attributes('disabled')).toBeDefined()
    await first.find('button[title="下移"]').trigger('click')
    expect(model.steps.map((s) => s.message.lit)).toEqual(['b', 'a', 'c'])
    stack.undo()
    expect(model.steps.map((s) => s.message.lit)).toEqual(['a', 'b', 'c'])
  })

  it('复制：新 uuid + undo 移除', async () => {
    const { wrapper, model, stack } = mountCard({ yaml: 'steps:\n  - log: a' })
    await wrapper.find('button[title="复制步骤"]').trigger('click')
    expect(model.steps).toHaveLength(2)
    expect(model.steps[1].uuid).not.toBe(model.steps[0].uuid)
    expect(model.steps[1].message.lit).toBe('a')
    stack.undo()
    expect(model.steps).toHaveLength(1)
  })

  it('删除选中卡 → emit select(null)', async () => {
    const { wrapper, model } = mountCard({ yaml: 'steps:\n  - log: a' })
    await wrapper.setProps({ selectedUuid: model.steps[0].uuid })
    await wrapper.find('button[title="删除步骤"]').trigger('click')
    expect(model.steps).toHaveLength(0)
    expect(wrapper.emitted('select')[0]).toEqual([null])
  })
})

describe('StepCard：嵌套分支容器入口', () => {
  it('find 卡片内嵌 then/else 容器（一层内嵌）', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: 'steps:\n  - find: a.png\n    then:\n      - log: hit\n    else:\n      - log: miss',
    })
    const card = await expandCard(wrapper, model.steps[0].uuid)
    const containers = card.findAll('.branch-container')
    expect(containers.length).toBe(2)
    expect(containers[0].text()).toContain('命中后')
    expect(containers[0].text()).toContain('记录日志 hit')
    await containers[0].find('button[title="删除步骤"]').trigger('click')
    expect(model.steps[0].then).toHaveLength(0)
    stack.undo()
    expect(model.steps[0].then).toHaveLength(1)
  })

  it('match 候选分支容器 + else', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'steps:\n  - match:\n    - a.png:\n      - log: hit\n    else:\n      - log: miss',
    })
    const card = await expandCard(wrapper, model.steps[0].uuid)
    const containers = card.findAll('.branch-container')
    expect(containers.length).toBe(2)
    expect(containers[0].text()).toContain('命中 a.png')
    expect(containers[1].text()).toContain('都未命中')
  })
})
