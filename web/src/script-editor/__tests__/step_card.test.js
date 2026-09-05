// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { makeStep } from '../factories'
import { STEP_KINDS } from '../model'
import { stepSummary } from '../components/kinds'
import StepCard from '../components/StepCard.vue'
import StepCanvas from '../components/StepCanvas.vue'
import { expandCard, setupScript } from './component_helpers'

/**
 * StepCard（v3 19 类）：收起态摘要、展开态强类型控件经 CommandStack 生效、
 * 字段错误按 Diagnostic.field 标红、选中高亮、上移/下移/复制/删除。
 */

const YAML_BY_KIND = {
  app_start: 'app.start',
  app_stop: 'app.stop',
  tap: 'tap: [0.5, 0.5]',
  swipe: 'swipe: {from: [0.3, 0.5], to: [0.7, 0.5], duration: 500ms}',
  key: 'key: BACK',
  text: 'text: ""',
  log: 'log: ""',
  wait: 'wait: 1s',
  set: 'set: {name: "", value: ""}',
  if: 'if: {cond: true, then: [], else: []}',
  loop: 'loop: {times: 3, steps: []}',
  break: 'break',
  call: 'call: {target: ""}',
  invoke: 'invoke: {capability: ""}',
  return: 'return: null',
  throw: "throw: ''",
  find: 'find: {template: ""}',
  match_first: 'match_first: {candidates: [{template: ""}]}',
  check: 'check: {template: ""}',
}

const SUMMARY_BY_KIND = {
  app_start: '启动当前应用',
  app_stop: '关闭当前应用',
  tap: '点击坐标 0.5, 0.5',
  swipe: '从 0.3, 0.5 滑到 0.7, 0.5 · 500ms',
  key: '按键 BACK',
  text: '输入文本',
  log: '记录日志',
  wait: '等待 1s',
  set: '设置 （未命名） = ?',
  if: '如果 true',
  loop: '循环 3 次',
  break: '跳出循环',
  call: '调用 （未填目标）',
  invoke: '调用能力 （未填能力）',
  return: '返回 ?',
  throw: '终止：（无原因）',
  find: '等待 （未选模板） 并执行命中后步骤',
  match_first: '按顺序匹配 1 个模板（首个命中获胜）',
  check: '检查 （未选模板）',
}

function mountCard({ yaml = 'version: 3\nsteps:\n  - log: hello\n', index = 0, props = {} } = {}) {
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

describe('StepCard：收起态摘要（v3 19 类全覆盖）', () => {
  for (const kind of STEP_KINDS) {
    it(`${kind} 摘要`, () => {
      expect(stepSummary(makeStep(kind))).toBe(SUMMARY_BY_KIND[kind])
      const { wrapper } = mountCard({ yaml: `version: 3\nsteps:\n  - ${YAML_BY_KIND[kind]}` })
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
    const created = setupScript('version: 3\nsteps:\n  - log: hello\n')
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
    const { wrapper, model, stack } = mountCard({ yaml: 'version: 3\nsteps:\n  - tap: [0.5, 0.8]' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="坐标X"]').setValue('0.2')
    expect(model.steps[0].at.lit).toEqual([0.2, 0.8])
    stack.undo()
    expect(model.steps[0].at.lit).toEqual([0.5, 0.8])
  })

  it('key 枚举下拉 + 方式选择', async () => {
    const { wrapper, model } = mountCard({ yaml: 'version: 3\nsteps:\n  - key: BACK' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('select[aria-label="按键"]').setValue('HOME')
    expect(model.steps[0].key.lit).toBe('HOME')
    await wrapper.find('select[aria-label="按键方式"]').setValue('down')
    expect(model.steps[0].action).toBe('down')
    await wrapper.find('select[aria-label="按键方式"]').setValue('press')
    expect(model.steps[0].action).toBeNull() // press = 缺省，不落 YAML
  })

  it('wait 随机区间开关（{min, max}）', async () => {
    const { wrapper, model } = mountCard({ yaml: 'version: 3\nsteps:\n  - wait: 1s' })
    await expandCard(wrapper, model.steps[0].uuid)
    const toggle = wrapper.find('input[type="checkbox"]')
    await toggle.setValue(true)
    expect(model.steps[0].max).toEqual({ lit: '1s' })
    await toggle.setValue(false)
    expect(model.steps[0].max).toBeNull()
  })

  it('find：模板/超时/阈值/保存结果/二次验证', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'version: 3\nsteps:\n  - find: {template: login.png}',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="模板"]').setValue('account.png')
    expect(model.steps[0].template.lit).toBe('account.png')
    const checkByText = (text) =>
      wrapper.findAll('label.field-check').find((l) => l.text().includes(text)).find('input[type="checkbox"]')
    await checkByText('等待超时').setValue(true)
    expect(model.steps[0].timeout).toEqual({ lit: '30s' })
    await checkByText('匹配阈值').setValue(true)
    expect(model.steps[0].threshold).toBe(0.85)
    await checkByText('保存结果').setValue(true)
    expect(model.steps[0].save).toBe('')
    await wrapper.find('input[aria-label="保存变量名"]').setValue('reward')
    expect(model.steps[0].save).toBe('reward')
    await checkByText('二次验证').setValue(true)
    expect(model.steps[0].verify).toEqual({ template: { lit: '' }, timeout: { lit: '5s' } })
    await wrapper.find('input[aria-label="验证模板"]').setValue('home.png')
    expect(model.steps[0].verify.template).toEqual({ lit: 'home.png' })
  })

  it('find 超时未勾选展示默认行为提示，勾选后隐藏', async () => {
    const checkboxByText = (wrapper, text) =>
      wrapper.findAll('label.field-check').find((l) => l.text().includes(text)).find('input[type="checkbox"]')
    const find = mountCard({ yaml: 'version: 3\nsteps:\n  - find: {template: login.png}' })
    await expandCard(find.wrapper, find.model.steps[0].uuid)
    expect(find.wrapper.text()).toContain('默认 30min')
    await checkboxByText(find.wrapper, '等待超时').setValue(true)
    expect(find.wrapper.text()).not.toContain('默认 30min')
    expect(find.model.steps[0].timeout).toEqual({ lit: '30s' })
  })

  it('check 默认 5s 提示 + 阈值', async () => {
    const check = mountCard({ yaml: 'version: 3\nsteps:\n  - check: {template: login.png}' })
    await expandCard(check.wrapper, check.model.steps[0].uuid)
    expect(check.wrapper.text()).toContain('默认 5s')
    await check.wrapper.findAll('label.field-check').find((l) => l.text().includes('检测超时')).find('input[type="checkbox"]').setValue(true)
    expect(check.model.steps[0].timeout).toEqual({ lit: '5s' })
    await check.wrapper.findAll('label.field-check').find((l) => l.text().includes('匹配阈值')).find('input[type="checkbox"]').setValue(true)
    expect(check.model.steps[0].threshold).toBe(0.85)
  })

  it('match_first：候选增删与阈值', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: 'version: 3\nsteps:\n  - match_first: {candidates: [{template: a.png}]}\n',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('button[title="添加候选"]').trigger('click')
    expect(model.steps[0].candidates).toHaveLength(2)
    await wrapper.find('button[title="删除候选"]').trigger('click')
    expect(model.steps[0].candidates).toHaveLength(1)
    // 候选阈值
    const candBlock = wrapper.find('.cand-block')
    await candBlock.findAll('label.field-check').find((l) => l.text().includes('阈值')).find('input[type="checkbox"]').setValue(true)
    expect(model.steps[0].candidates[0].threshold).toBe(0.85)
    stack.undo()
    expect(model.steps[0].candidates[0].threshold).toBeNull()
  })

  it('if：字面量 ↔ $引用切换（v3 表达式不按类型过滤）', async () => {
    const created = setupScript("version: 3\nparams:\n  - 'boolean:enable:是否启用'\nsteps:\n  - if: {cond: true, then: [], else: []}")
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
    expect(model.steps[0].cond).toEqual({ lit: '' })
  })

  it('loop：无限循环开关', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'version: 3\nsteps:\n  - loop: {times: 3, steps: [{log: x}]}\n',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    const infinite = wrapper.findAll('label.field-check').find((l) => l.text().includes('无限循环')).find('input[type="checkbox"]')
    await infinite.setValue(true)
    expect(model.steps[0].times).toBeNull()
    await infinite.setValue(false)
    expect(model.steps[0].times).toEqual({ lit: 3 })
  })

  it('set：变量名 + 取值', async () => {
    const { wrapper, model } = mountCard({ yaml: 'version: 3\nsteps:\n  - set: {name: a, value: 1}' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="变量名"]').setValue('total')
    expect(model.steps[0].name).toBe('total')
    await wrapper.find('input[aria-label="取值"]').setValue('5')
    expect(model.steps[0].value).toEqual({ lit: 5 })
  })

  it('call 自由输入目标与 with 增删', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'version: 3\nsteps:\n  - call: {target: "script:sub", with: {enable: true}}\n',
    })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="调用目标"]').setValue('function:common/login')
    expect(model.steps[0].target).toBe('function:common/login')
    await wrapper.find('button[title="添加实参"]').trigger('click')
    expect(Object.keys(model.steps[0].with)).toEqual(['enable', 'param1'])
    await wrapper.find('button[title="删除实参"]').trigger('click')
    expect(Object.keys(model.steps[0].with)).toEqual(['param1'])
  })

  it('throw 原因表达式', async () => {
    const { wrapper, model } = mountCard({ yaml: "version: 3\nsteps:\n  - throw: 出错了" })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[aria-label="终止原因"]').setValue('again')
    expect(model.steps[0].message).toEqual({ lit: 'again' })
  })

  it('app.start 指定包名开关', async () => {
    const { wrapper, model } = mountCard({ yaml: 'version: 3\nsteps:\n  - app.start' })
    await expandCard(wrapper, model.steps[0].uuid)
    await wrapper.find('input[type="checkbox"]').setValue(true)
    expect(model.steps[0].package).toEqual({ lit: '' })
    await wrapper.find('input[aria-label="应用包名"]').setValue('com.example.app')
    expect(model.steps[0].package).toEqual({ lit: 'com.example.app' })
  })
})

describe('StepCard：错误标红定位', () => {
  it('Diagnostic.field → 控件红框 + 卡片错误徽标', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'version: 3\nsteps:\n  - tap: [0.5, 0.8]',
      props: {
        diagnostics: [
          { code: 'yaml.v3.coord.range', step_path: 'steps[0]', field: 'at', message: '坐标超出 0~1' },
          { code: 'yaml.v3.field.missing', step_path: 'steps[1]', field: 'at', message: '别的卡片的错误' },
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
      yaml: 'version: 3\nsteps:\n  - tap: [0.5, 0.8]',
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
      yaml: 'version: 3\nsteps:\n  - log: a\n  - log: b\n  - log: c',
    })
    const first = wrapper.findComponent(StepCard)
    expect(first.find('button[title="上移"]').attributes('disabled')).toBeDefined()
    await first.find('button[title="下移"]').trigger('click')
    expect(model.steps.map((s) => s.message.lit)).toEqual(['b', 'a', 'c'])
    stack.undo()
    expect(model.steps.map((s) => s.message.lit)).toEqual(['a', 'b', 'c'])
  })

  it('复制：新 uuid + undo 移除', async () => {
    const { wrapper, model, stack } = mountCard({ yaml: 'version: 3\nsteps:\n  - log: a' })
    await wrapper.find('button[title="复制步骤"]').trigger('click')
    expect(model.steps).toHaveLength(2)
    expect(model.steps[1].uuid).not.toBe(model.steps[0].uuid)
    expect(model.steps[1].message.lit).toBe('a')
    stack.undo()
    expect(model.steps).toHaveLength(1)
  })

  it('删除选中卡 → emit select(null)', async () => {
    const { wrapper, model } = mountCard({ yaml: 'version: 3\nsteps:\n  - log: a' })
    await wrapper.setProps({ selectedUuid: model.steps[0].uuid })
    await wrapper.find('button[title="删除步骤"]').trigger('click')
    expect(model.steps).toHaveLength(0)
    expect(wrapper.emitted('select')[0]).toEqual([null])
  })
})

describe('StepCard：拖动排序', () => {
  function transfer() {
    const data = new Map()
    return {
      setData(type, value) { data.set(type, value) },
      getData(type) { return data.get(type) ?? '' },
      effectAllowed: '',
      dropEffect: '',
    }
  }

  it('拖动手柄调整同列表顺序，并可撤销', async () => {
    const created = setupScript('version: 3\nsteps:\n  - log: a\n  - log: b\n  - log: c')
    const wrapper = mount(StepCanvas, { props: { model: created.model, stack: created.stack } })
    const source = created.model.steps[0]
    const target = created.model.steps[2]
    const dt = transfer()
    const targetCard = wrapper.find(`[data-step-uuid="${target.uuid}"]`)
    targetCard.element.getBoundingClientRect = () => ({ top: 0, bottom: 100, height: 100 })

    await wrapper.find(`[data-step-uuid="${source.uuid}"] .drag-handle`).trigger('dragstart', { dataTransfer: dt })
    await targetCard.trigger('dragover', { dataTransfer: dt, clientY: 90 })
    await targetCard.trigger('drop', { dataTransfer: dt, clientY: 90 })

    expect(created.model.steps.map((s) => s.message.lit)).toEqual(['b', 'c', 'a'])
    created.stack.undo()
    expect(created.model.steps.map((s) => s.message.lit)).toEqual(['a', 'b', 'c'])
  })

  it('拖入空分支时移动到该分支末尾', async () => {
    const created = setupScript('version: 3\nsteps:\n  - log: outside\n  - if: {cond: true, then: [], else: []}')
    const wrapper = mount(StepCanvas, { props: { model: created.model, stack: created.stack } })
    const source = created.model.steps[0]
    const ifStep = created.model.steps[1]
    await wrapper.find(`[data-step-uuid="${ifStep.uuid}"] button[title="展开编辑"]`).trigger('click')
    const dt = transfer()
    await wrapper.find(`[data-step-uuid="${source.uuid}"] .drag-handle`).trigger('dragstart', { dataTransfer: dt })
    const empty = wrapper.find(`[data-step-uuid="${ifStep.uuid}"] .branch-container .branch-empty`)
    await empty.trigger('dragover', { dataTransfer: dt })
    await empty.trigger('drop', { dataTransfer: dt })

    expect(created.model.steps).toHaveLength(1)
    expect(created.model.steps[0]).toBe(ifStep)
    expect(ifStep.then.map((s) => s.message.lit)).toEqual(['outside'])
  })
})

describe('StepCard：嵌套分支容器入口', () => {
  it('find 卡片内嵌 then/else 容器（一层内嵌）', async () => {
    const { wrapper, model, stack } = mountCard({
      yaml: 'version: 3\nsteps:\n  - find: {template: a.png, then: [{log: hit}], else: [{log: miss}]}',
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

  it('match_first 候选分支容器 + else', async () => {
    const { wrapper, model } = mountCard({
      yaml: 'version: 3\nsteps:\n  - match_first: {candidates: [{template: a.png, steps: [{log: hit}]}], else: [{log: miss}]}',
    })
    const card = await expandCard(wrapper, model.steps[0].uuid)
    const containers = card.findAll('.branch-container')
    expect(containers.length).toBe(2)
    expect(containers[0].text()).toContain('命中 a.png')
    expect(containers[1].text()).toContain('都未命中')
  })
})
