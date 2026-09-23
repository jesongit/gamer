// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { createCall } from '../factories'
import { SE_TARGET_OPTIONS } from '../targets'
import { stepSummary } from '../components/kinds'
import StepCard from '../components/StepCard.vue'
import StepCanvas from '../components/StepCanvas.vue'
import { expandCard, setupScript, setupFunctions } from './component_helpers'

/**
 * StepCard（V1 四类）：收起态摘要、展开态控件经 CommandStack 生效、
 * 字段错误按 Diagnostic.field 标红、整行展开、选中高亮、运行/复制/删除。
 */

const YAML_BY_KIND = {
  call: 'run:\n  - tap: [0.5, 0.5]\n',
  if: 'run:\n  - if: $flag\n    then: []\n    else: []\n',
  repeat: 'run:\n  - repeat: 3\n    do: []\n',
  return: 'run:\n  - return: null\n',
}

const SUMMARY_BY_KIND = {
  call: '点击 · (0.5, 0.5)',
  if: '如果 · $flag',
  repeat: '重复 · 3 次',
  return: '返回 · null',
}

function mountCard({ yaml = 'run:\n  - log: hello\n', index = 0, props = {} } = {}) {
  const created = setupScript(yaml)
  const wrapper = mount(StepCard, {
    props: {
      model: created.model,
      stack: created.stack,
      step: created.model.run[index],
      containerPath: ['run'],
      basePath: 'run',
      index,
      ...props,
    },
  })
  return { ...created, wrapper }
}

describe('StepCard：收起态摘要（V1 四类）', () => {
  for (const kind of Object.keys(YAML_BY_KIND)) {
    it(`${kind} 摘要`, () => {
      const created = setupScript(YAML_BY_KIND[kind])
      allocate(created)
      expect(stepSummary(created.model.run[0])).toBe(SUMMARY_BY_KIND[kind])
    })
  }

  function allocate(created) {
    // 摘要不依赖 uuid；直接调用
    void created
  }
})

describe('StepCard：V1 交互', () => {
  it('call 卡：函数名和同行返回值输入经命令栈生效', async () => {
    const created = setupScript('run:\n  - tap: [0.5, 0.5]\n')
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[0],
        containerPath: ['run'], basePath: 'run', index: 0,
      },
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    const fnInput = wrapper.find('input[aria-label="函数名"]')
    await fnInput.setValue('wait_find')
    expect(created.model.run[0].fn).toBe('wait_find')
    expect(wrapper.find('input[type="checkbox"]').exists()).toBe(false)
    expect(wrapper.find('.return-value-options').exists()).toBe(false)
    expect(wrapper.find('.call-signature input[aria-label="返回值变量名"]').exists()).toBe(true)
    await wrapper.find('input[aria-label="返回值变量名"]').setValue('hit')
    expect(created.model.run[0].as).toBe('hit')
    expect(wrapper.get('input[aria-label="返回值变量名"]').element.value).toBe('hit')
    await wrapper.get('[aria-label="清除返回值变量"]').trigger('click')
    expect(created.model.run[0].as).toBeNull()
    created.stack.undo()
    expect(created.model.run[0].as).toBe('hit')
    wrapper.unmount()
  })

  it('call 卡：Schema 驱动的命名参数编辑（宿主注入 resolveParamsSync）', async () => {
    const created = setupScript('run:\n  - wait_find: {}\n')
    const schemaParams = [
      { name: 'template', type: 'template', required: true, default: null, desc: '' },
      { name: 'timeout', type: 'duration', required: false, default: '30s', desc: '' },
    ]
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[0],
        containerPath: ['run'], basePath: 'run', index: 0,
      },
      global: {
        provide: {
          [SE_TARGET_OPTIONS]: {
            targets: [{ target: 'wait_find', group: 'plugin' }],
            resolveParams: async () => schemaParams,
            resolveParamsSync: () => schemaParams,
          },
        },
      },
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    expect(wrapper.get('[data-arg-name="template"]').find('[aria-label="必填"]').exists()).toBe(true)
    expect(wrapper.find('button[data-param="template"]').exists()).toBe(false)
    expect(wrapper.find('input[aria-label="参数 timeout数值"]').exists()).toBe(false)
    expect(wrapper.get('button[data-param="timeout"]').text()).toContain('使用默认值：')
    expect(wrapper.get('button[data-param="timeout"]').text()).toContain('30s')
    await wrapper.get('button[data-param="timeout"]').trigger('click')
    expect(created.model.run[0].args.entries.timeout).toEqual({ lit: '30s' })
    await wrapper.get('button[aria-label="恢复 timeout 默认值"]').trigger('click')
    expect(created.model.run[0].args.entries).not.toHaveProperty('timeout')
    await wrapper.get('input[aria-label="参数 template"]').setValue('button.png')
    expect(created.model.run[0].args).toEqual({
      kind: 'map',
      entries: { template: { lit: 'button.png' } },
    })
    wrapper.unmount()
  })

  it('if 卡：条件编辑生效（引用模式输入路径）', async () => {
    const created = setupScript('run:\n  - if: $flag\n    then:\n      - log: yes\n')
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[0],
        containerPath: ['run'], basePath: 'run', index: 0,
      },
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    // 切到引用模式，再输入路径
    const refBtn = wrapper.findAll('.mode-btn').find((b) => b.text() === '引用')
    await refBtn.trigger('click')
    const refInput = wrapper.find('input[aria-label="条件引用"]')
    await refInput.setValue('other')
    expect(created.model.run[0].cond).toEqual({ ref: 'other' })
    wrapper.unmount()
  })

  it('repeat 卡：次数编辑生效（引用模式输入路径）', async () => {
    const created = setupScript('run:\n  - repeat: 3\n    do:\n      - log: t\n')
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[0],
        containerPath: ['run'], basePath: 'run', index: 0,
      },
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    const refBtn = wrapper.findAll('.mode-btn').find((b) => b.text() === '引用')
    await refBtn.trigger('click')
    const refInput = wrapper.find('input[aria-label="次数引用"]')
    await refInput.setValue('count')
    expect(created.model.run[0].times).toEqual({ ref: 'count' })
    wrapper.unmount()
  })

  it('字段错误按 field 标红（err-badge 数量）', () => {
    const created = setupScript('run:\n  - tap: [0.5, 0.5]\n')
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[0],
        containerPath: ['run'], basePath: 'run', index: 0,
        diagnostics: [{ code: 'yaml.fn.not_found', step_path: 'run[0]', field: 'fn', message: '函数不存在' }],
      },
    })
    expect(wrapper.find('.err-badge').exists()).toBe(true)
    wrapper.unmount()
  })

  it('操作区仅保留运行、复制、删除，按钮不触发展开，复制删除经命令栈', async () => {
    const created = setupScript('run:\n  - log: a\n  - log: b\n  - log: c\n')
    const wrapper = mount(StepCard, {
      props: {
        model: created.model, stack: created.stack, step: created.model.run[1],
        containerPath: ['run'], basePath: 'run', index: 1, testFrom: true,
      },
    })
    expect(wrapper.findAll('.head-actions button').map(b => b.text())).toEqual(['运行', '复制', '删除'])
    await wrapper.get('button[title="从此步骤运行"]').trigger('click')
    expect(wrapper.emitted('test-from')[0][0]).toBe(created.model.run[1].uuid)
    await wrapper.find('button[title="复制步骤"]').trigger('click')
    expect(created.model.run).toHaveLength(4)
    await wrapper.find('button[title="删除步骤"]').trigger('click')
    expect(created.model.run).toHaveLength(3)
    expect(wrapper.emitted('toggle-expand')).toBeUndefined()
    wrapper.unmount()
  })

  it('点击行标题或空白展开收起，编辑控件和拖动手柄不触发折叠', async () => {
    const {wrapper} = mountCard()
    await wrapper.get('.summary').trigger('click')
    expect(wrapper.find('.card-body').exists()).toBe(true)
    expect(wrapper.get('.expand-btn').attributes('aria-expanded')).toBe('true')
    await wrapper.get('input[aria-label="函数名"]').trigger('click')
    await wrapper.get('.drag-handle').trigger('click')
    expect(wrapper.emitted('toggle-expand')).toHaveLength(1)
    await wrapper.get('.card-head').trigger('click')
    expect(wrapper.find('.card-body').exists()).toBe(false)
    expect(wrapper.get('.expand-btn').attributes('aria-expanded')).toBe('false')
    wrapper.unmount()
  })
})

describe('StepCanvas：V1 装配', () => {
  it('渲染卡片 + 添加步骤入口 + 选中联动', async () => {
    const created = setupScript('run:\n  - log: a\n  - log: b\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, templates: [] },
    })
    expect(wrapper.findAll('[data-step-uuid]')).toHaveLength(2)
    await wrapper.find('[data-step-uuid]').trigger('click')
    expect(wrapper.emitted('select')[0][0]).toBe(created.model.run[0].uuid)
    expect(wrapper.find('.add-btn').exists()).toBe(true)
    wrapper.unmount()
  })

  it('函数库：函数下拉切换容器', async () => {
    const created = setupFunctions('functions:\n  a:\n    run:\n      - log: x\n  b:\n    run:\n      - log: y\n')
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack, initialFn: 'a', hideFunctionToolbar: true },
    })
    expect(wrapper.find('.add-btn').exists()).toBe(false)
    wrapper.unmount()
  })
  it('createCall 工厂与画布兼容（插入后渲染）', async () => {
    const created = setupScript('run: []\n')
    created.stack.apply({ type: 'insert_step', path: ['run'], index: 0, step: createCall('tap') })
    const wrapper = mount(StepCanvas, {
      props: { model: created.model, stack: created.stack },
    })
    expect(wrapper.findAll('[data-step-uuid]')).toHaveLength(1)
    wrapper.unmount()
  })
})

// 局部引入 setupFunctions（component_helpers 未导出时的兜底导入点）
import { setupFunctions } from './component_helpers'
