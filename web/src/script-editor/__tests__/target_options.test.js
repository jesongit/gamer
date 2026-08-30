// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import StepCard from '../components/StepCard.vue'
import StepCanvas from '../components/StepCanvas.vue'
import { SE_TARGET_OPTIONS } from '../targets'
import { setupScript } from './component_helpers'

/**
 * call/func 目标下拉（宿主 provide SE_TARGET_OPTIONS）与 args 自动生成：
 * - call：单下拉选脚本 → resolveParams 拉声明 → args 整表生成（默认值预填、必填补类型空值）；
 * - func：文件 + 函数名两级下拉 → 两级都选中才下发 target；
 * - 未注入（如独立挂载测试）回退自由文本输入框（step_card.test.js 覆盖）。
 */

function makeOptions(overrides = {}) {
  return {
    callScripts: [
      { target: 'sub.yaml', label: 'sub' },
      { target: 'bad.yaml' }, // resolveParams 拒绝 → 只改 target 不动 args 的分支
    ],
    funcFiles: [
      { file: 'common', functions: ['login'] },
      { file: 'ui', functions: ['登记'] },
    ],
    resolveParams: async (kind, target) => {
      if (kind === 'call' && target === 'sub.yaml') {
        return [
          { type: 'text', name: 'account', remark: '账号', default: 'abc' },
          { type: 'bool', name: 'enable', remark: '开关', default: null },
        ]
      }
      if (kind === 'func' && target === 'ui/登记') {
        return [{ type: 'time', name: 'wait', remark: '等待', default: '3s' }]
      }
      if (kind === 'call' && target === 'bad.yaml') {
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
      params: created.model.params,
    },
    global: { provide: { [SE_TARGET_OPTIONS]: options } },
  })
  return { ...created, wrapper, async expand() { await wrapper.find('button[title="展开编辑"]').trigger('click') } }
}

async function flush() {
  await nextTick()
  await nextTick()
  await new Promise((r) => setTimeout(r, 0))
  await nextTick()
}

describe('call 目标下拉 + args 自动生成', () => {
  it('选脚本后按声明生成 args：默认值预填、必填填类型空值', async () => {
    const { wrapper, model } = mountCard(
      'steps:\n  - call: old.yaml\n    args:\n      legacy: "1"',
      makeOptions(),
    )
    await wrapper.find('button[title="展开编辑"]').trigger('click')
    await wrapper.find('select[aria-label="目标脚本"]').setValue('sub.yaml')
    await flush()
    expect(model.steps[0].target).toBe('sub.yaml')
    expect(model.steps[0].args).toEqual({
      account: { lit: 'abc' },
      enable: { lit: false },
    })
  })

  it('解析失败（null）只改 target 不动 args；一次撤销回退整体', async () => {
    const { wrapper, model, stack } = mountCard(
      'steps:\n  - call: old.yaml\n    args:\n      keep: "1"',
      makeOptions(),
    )
    await wrapper.find('button[title="展开编辑"]').trigger('click')
    await wrapper.find('select[aria-label="目标脚本"]').setValue('sub.yaml')
    await flush()
    expect(model.steps[0].args).toEqual({ account: { lit: 'abc' }, enable: { lit: false } })
    // 解析拒绝 → 只下发 target，args 保持不动
    await wrapper.find('select[aria-label="目标脚本"]').setValue('bad.yaml')
    await flush()
    expect(model.steps[0].target).toBe('bad.yaml')
    expect(model.steps[0].args).toEqual({ account: { lit: 'abc' }, enable: { lit: false } })
    // 每次目标变更各自成一条撤销记录：撤两次回到初始 old.yaml + 原 args
    expect(stack.undo()).toBe(true)
    expect(model.steps[0].target).toBe('sub.yaml')
    expect(stack.undo()).toBe(true)
    expect(model.steps[0].target).toBe('old.yaml')
    expect(model.steps[0].args).toEqual({ keep: { lit: '1' } })
  })
})

describe('func 文件 + 函数名两级下拉', () => {
  it('两级选中才下发 target，args 按函数声明生成', async () => {
    const { wrapper, model } = mountCard('steps:\n  - func: common/login', makeOptions())
    await wrapper.find('button[title="展开编辑"]').trigger('click')
    const fileSel = wrapper.find('select[aria-label="函数库文件"]')
    const fnSel = wrapper.find('select[aria-label="函数名"]')
    expect(fileSel.element.value).toBe('common')
    expect(fnSel.element.value).toBe('login')
    // 换文件：函数选择复位，target 暂不下发
    await fileSel.setValue('ui')
    expect(fnSel.element.value).toBe('')
    expect(model.steps[0].target).toBe('common/login')
    // 选中函数名 → target + args 一起下发
    await fnSel.setValue('登记')
    await flush()
    expect(model.steps[0].target).toBe('ui/登记')
    expect(model.steps[0].args).toEqual({ wait: { lit: '3s' } })
  })
})

describe('新添加步骤自动展开', () => {
  it('面板插入后新卡片即为展开态', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(StepCanvas, {
      props: {
        model: created.model,
        stack: created.stack,
        diagnostics: [],
      },
    })
    await wrapper.find('.add-btn').trigger('click')
    await wrapper.findAll('.entry-btn')[0].trigger('click') // 启动应用
    const uuid = created.model.steps[1].uuid
    const card = wrapper.find(`[data-step-uuid="${uuid}"]`)
    expect(card.classes()).toContain('expanded')
    expect(card.find('.card-body').exists()).toBe(true)
  })
})
