// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { mount } from '@vue/test-utils'
import { SE_TARGET_OPTIONS } from './targets'
import { parseScript } from './codec'
import AddStepPanel from './components/AddStepPanel.vue'
import CellEditor from './components/CellEditor.vue'
import StepCard from './components/StepCard.vue'
import { initializeArgsFromSchema } from './factories'
import { CommandStack } from './commands'
import { expandCard } from './__tests__/component_helpers'

const SCHEMA = [
  { name: 'template', type: 'template', required: true, default: null, desc: '' },
  { name: 'enabled', type: 'boolean', required: false, default: false, desc: '' },
]

function setup(yaml) {
  const { model } = parseScript(yaml)
  return { model, stack: new CommandStack(model) }
}

function mountCard(created, targetOptions, index = 0) {
  return mount(StepCard, {
    props: {
      model: created.model,
      stack: created.stack,
      step: created.model.run[index],
      containerPath: ['run'],
      basePath: 'run',
      index,
    },
    global: { provide: { [SE_TARGET_OPTIONS]: targetOptions } },
  })
}

async function settle() {
  await nextTick()
  await Promise.resolve()
  await nextTick()
}

describe('P3-STEP：Schema 初始化、类型控件、参数形态和请求代次', () => {
  it('A01：函数切换只初始化必填项，false 默认值留在按钮中', async () => {
    const created = setup('run:\n  - old: {}\n')
    const wrapper = mountCard(created, {
      targets: [{ target: 'demo', group: 'plugin' }],
      resolveParams: async () => SCHEMA,
      resolveParamsSync: () => SCHEMA,
    })

    await expandCard(wrapper, created.model.run[0].uuid)
    await wrapper.find('select[aria-label="函数"]').setValue('demo')
    await settle()

    expect(created.model.run[0].args).toEqual({
      kind: 'map',
      entries: {
        template: { lit: null, missing: true },
      },
    })
    expect(wrapper.find('.arg-row').exists()).toBe(true)
    wrapper.unmount()
  })

  it('A02：CellEditor 直接接受正式 V1 类型并保持布尔、列表、对象真实类型', async () => {
    const booleanEditor = mount(CellEditor, { props: { cell: { lit: false }, type: 'boolean', label: '开关' } })
    await booleanEditor.find('select[aria-label="开关"]').setValue('true')
    expect(booleanEditor.emitted('change').at(-1)).toEqual([{ lit: true }])

    const listEditor = mount(CellEditor, { props: { cell: { lit: ['old'] }, type: 'list', label: '列表' } })
    expect(listEditor.find('textarea[aria-label="列表"]').exists()).toBe(true)
    await listEditor.find('textarea[aria-label="列表"]').setValue('["new"]')
    expect(listEditor.emitted('change').at(-1)).toEqual([{ lit: ['new'] }])

    const objectEditor = mount(CellEditor, { props: { cell: { lit: { old: true } }, type: 'object', label: '对象' } })
    await objectEditor.find('textarea[aria-label="对象"]').setValue('{"enabled":true}')
    expect(objectEditor.emitted('change').at(-1)).toEqual([{ lit: { enabled: true } }])
    booleanEditor.unmount()
    listEditor.unmount()
    objectEditor.unmount()
  })

  it('A03：添加步骤与函数切换共用 Schema 初始化', async () => {
    const created = setup('run: []\n')
    const wrapper = mount(AddStepPanel, {
      props: { stack: created.stack, anchor: { containerPath: ['run'], index: 0 } },
      global: {
        provide: {
          [SE_TARGET_OPTIONS]: {
            targets: [{ target: 'demo', group: 'plugin' }],
            resolveParams: async () => SCHEMA,
          },
        },
      },
    })

    await wrapper.find('button[data-kind="call:demo"]').trigger('click')
    await settle()

    expect(created.model.run[0].args).toEqual(initializeArgsFromSchema(SCHEMA))
    wrapper.unmount()
  })

  it('A04：单值参数按名称显示，添加默认参数时保留其已有值', async () => {
    const created = setup('run:\n  - wait_find: login.png\n')
    const schema = [
      { name: 'template', type: 'template', required: true, default: null },
      { name: 'timeout', type: 'duration', required: false, default: '30s' },
    ]
    const wrapper = mountCard(created, {
      targets: [{ target: 'wait_find', group: 'plugin' }],
      resolveParams: async () => schema, resolveParamsSync: () => schema,
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    expect(wrapper.get('input[aria-label="参数 template"]').element.value).toBe('login.png')
    await wrapper.get('button[data-param="timeout"]').trigger('click')
    expect(created.model.run[0].args).toEqual({ kind: 'map', entries: {
      template: { lit: 'login.png' }, timeout: { lit: '30s' },
    } })
    await wrapper.get('button[aria-label="恢复 timeout 默认值"]').trigger('click')
    expect(created.model.run[0].args.entries).toEqual({ template: { lit: 'login.png' } })
    wrapper.unmount()
  })

  it('A05：旧 Schema 响应不能覆盖最后一次函数选择', async () => {
    const created = setup('run:\n  - old: {}\n')
    const deferred = {}
    const resolveParams = vi.fn((fn) => new Promise((resolve) => { deferred[fn] = resolve }))
    const wrapper = mountCard(created, {
      targets: [
        { target: 'fn_a', group: 'plugin' },
        { target: 'fn_b', group: 'plugin' },
      ],
      resolveParams,
      resolveParamsSync: () => null,
    })
    await expandCard(wrapper, created.model.run[0].uuid)
    const select = wrapper.find('select[aria-label="函数"]')

    await select.setValue('fn_a')
    await select.setValue('fn_b')
    expect(created.model.run[0].fn).toBe('fn_b')

    deferred.fn_a([{ name: 'a', type: 'string', required: true, default: null, desc: '' }])
    await settle()
    expect(created.model.run[0].fn).toBe('fn_b')
    expect(created.model.run[0].args).toEqual({ kind: 'map', entries: {} })

    deferred.fn_b([{ name: 'b', type: 'string', required: true, default: null, desc: '' }])
    await settle()
    expect(created.model.run[0].fn).toBe('fn_b')
    expect(created.model.run[0].args).toEqual({ kind: 'map', entries: { b: { lit: null, missing: true } } })
    wrapper.unmount()
  })
})
