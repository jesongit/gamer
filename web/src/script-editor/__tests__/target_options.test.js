// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import StepCard from '../components/StepCard.vue'
import StepCanvas from '../components/StepCanvas.vue'
import { SE_TARGET_OPTIONS, splitFunctionTarget } from '../targets'
import { setupScript } from './component_helpers'

/**
 * call 目标下拉（宿主 provide SE_TARGET_OPTIONS，v3 命名空间 target）与 with 自动生成：
 * - 单下拉选 script:/function: 目标 → resolveParams 拉声明 → with 整表生成（默认值预填）；
 * - 未注入（如独立挂载测试）回退自由文本输入框。
 */

function makeOptions(overrides = {}) {
  return {
    targets: [
      { target: 'script:sub', label: 'sub' },
      { target: 'script:bad' }, // resolveParams 拒绝 → 只改 target 不动 with 的分支
      { target: 'function:ui/登记', label: 'ui/登记' },
    ],
    resolveParams: async (target) => {
      if (target === 'script:sub') {
        return [
          { type: 'string', name: 'account', remark: '账号', default: 'abc', rawForm: false },
          { type: 'boolean', name: 'enable', remark: '开关', default: null, rawForm: false },
        ]
      }
      if (target === 'function:ui/登记') {
        return [{ type: 'integer', name: 'wait', remark: '等待', default: 3, rawForm: false }]
      }
      if (target === 'script:bad') {
        throw new Error('network down')
      }
      return null
    },
    resolveParamsSync: () => null,
    ...overrides,
  }
}

function mountCard(yaml, options) {
  const created = setupScript(yaml)
  const wrapper = mount(StepCard, {
    props: {
      model: created.model,
      stack: created.stack,
      step: created.model.steps[0],
      containerPath: ['steps'],
      basePath: 'steps',
      index: 0,
    },
    global: options ? { provide: { [SE_TARGET_OPTIONS]: options } } : undefined,
  })
  return { ...created, wrapper }
}

async function expand(wrapper, uuid) {
  const card = wrapper.find(`[data-step-uuid="${uuid}"]`)
  await card.find('button[title="展开编辑"]').trigger('click')
  return card
}

describe('call 目标下拉（v3 命名空间）', () => {
  it('注入候选时渲染分组下拉；选择 script: 目标后按声明生成 with', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - call: {target: ""}\n', makeOptions())
    await expand(wrapper, model.steps[0].uuid)
    const sel = wrapper.find('select[aria-label="调用目标"]')
    expect(sel.exists()).toBe(true)
    const groups = sel.findAll('optgroup')
    expect(groups.map((g) => g.attributes('label'))).toEqual(['脚本（script:）', '函数（function:）'])
    await sel.setValue('script:sub')
    await nextTick()
    await nextTick()
    expect(model.steps[0].target).toBe('script:sub')
    // with 按声明生成：有默认值填默认值，必填填类型空值
    expect(model.steps[0].with).toEqual({ account: { lit: 'abc' }, enable: { lit: false } })
  })

  it('选择 function: 目标同样生成 with；resolveParams 抛错只改 target', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - call: {target: ""}\n', makeOptions())
    await expand(wrapper, model.steps[0].uuid)
    const sel = wrapper.find('select[aria-label="调用目标"]')
    await sel.setValue('function:ui/登记')
    await nextTick()
    await nextTick()
    expect(model.steps[0].target).toBe('function:ui/登记')
    expect(model.steps[0].with).toEqual({ wait: { lit: 3 } })

    await sel.setValue('script:bad')
    await nextTick()
    await nextTick()
    expect(model.steps[0].target).toBe('script:bad')
    expect(model.steps[0].with).toEqual({ wait: { lit: 3 } }) // 解析失败 → with 保持原样
  })

  it('已失效目标在下拉中保留「（已失效）」选项', async () => {
    const { wrapper } = mountCard('version: 3\nsteps:\n  - call: {target: "script:gone"}\n', makeOptions())
    await expand(wrapper, wrapper.props('step').uuid)
    const sel = wrapper.find('select[aria-label="调用目标"]')
    expect(sel.element.value).toBe('script:gone')
    expect(sel.text()).toContain('已失效')
  })

  it('save 开关：勾选生成 save 字段，填名后下发', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - call: {target: "script:sub"}\n', makeOptions())
    await expand(wrapper, model.steps[0].uuid)
    const saveToggle = wrapper.findAll('label.field-check').find((l) => l.text().includes('保存返回值'))
    await saveToggle.find('input[type="checkbox"]').setValue(true)
    expect(model.steps[0].save).toBe('')
    await wrapper.find('input[aria-label="保存返回值变量名"]').setValue('result')
    expect(model.steps[0].save).toBe('result')
    // 取消勾选 → null
    await saveToggle.find('input[type="checkbox"]').setValue(false)
    expect(model.steps[0].save).toBeNull()
  })

  it('with 实参增删改（经命令栈）', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - call: {target: "script:sub", with: {account: "abc"}}\n', makeOptions())
    await expand(wrapper, model.steps[0].uuid)
    await wrapper.find('button[title="添加实参"]').trigger('click')
    expect(Object.keys(model.steps[0].with)).toEqual(['account', 'param1'])
    await wrapper.find('button[title="删除实参"]').trigger('click')
    expect(Object.keys(model.steps[0].with)).toEqual(['param1'])
  })
})

describe('未注入候选：自由文本输入', () => {
  it('call 目标回退 input，输入即下发 target', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - call: {target: ""}\n', null)
    await expand(wrapper, model.steps[0].uuid)
    const input = wrapper.find('input[aria-label="调用目标"]')
    expect(input.exists()).toBe(true)
    await input.setValue('script:other')
    expect(model.steps[0].target).toBe('script:other')
  })
})

describe('invoke 能力调用', () => {
  it('capability 输入 + with 增删', async () => {
    const { wrapper, model } = mountCard('version: 3\nsteps:\n  - invoke: {capability: "vision.match"}\n', null)
    await expand(wrapper, model.steps[0].uuid)
    const input = wrapper.find('input[aria-label="能力名"]')
    await input.setValue('input.tap')
    expect(model.steps[0].capability).toBe('input.tap')
    await wrapper.find('button[title="添加实参"]').trigger('click')
    expect(Object.keys(model.steps[0].with)).toEqual(['param1'])
  })
})

describe('画布集成：插入 call 卡片', () => {
  it('画布添加面板选「调用脚本/函数」→ 空目标 call 卡', async () => {
    const created = setupScript('version: 3\nsteps:\n  - log: a\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, context: 'script' },
      global: { provide: { [SE_TARGET_OPTIONS]: makeOptions() } },
    })
    await wrapper.find('button.add-btn').trigger('click')
    await wrapper.find('button[data-kind="call"]').trigger('click')
    expect(created.model.steps[1].kind).toBe('call')
    expect(created.model.steps[1].target).toBe('')
  })
})

describe('splitFunctionTarget', () => {
  it('文件短路径按最后一个 / 分割；非 function: target 返回 null', () => {
    expect(splitFunctionTarget('function:common/login')).toEqual(['common', 'login'])
    expect(splitFunctionTarget('function:common/login/is_logged_in')).toEqual(['common/login', 'is_logged_in'])
    expect(splitFunctionTarget('function:login')).toBeNull()
    expect(splitFunctionTarget('script:daily/x')).toBeNull()
    expect(splitFunctionTarget('bare')).toBeNull()
  })
})
