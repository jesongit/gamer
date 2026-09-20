// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ParamEditor from './components/ParamEditor.vue'
import ParamsForm from './components/ParamsForm.vue'
import { parseScript, serialize } from './codec'
import { setupScript } from './__tests__/component_helpers'

const decl = (name, type, value = null, required = false, extra = {}) => ({
  name, type, required, default: value, desc: '', ...extra,
})

function formRow(wrapper, name) {
  return wrapper.findAll('.pf-row').find((item) => item.text().includes(`$${name}`))
}

describe('P3-FORM / A06 参数值类型回归', () => {
  it('ParamsForm 的 list/object/any JSON 编辑保留数组、对象、字符串、布尔和 null', async () => {
    const wrapper = mount(ParamsForm, {
      props: {
        params: [
          decl('items', 'list', []),
          decl('options', 'object', {}),
          decl('value', 'any', null, false, { hasDefault: true }),
        ],
        initialArgs: {
          items: ['old'],
          options: { enabled: false },
          value: 'old',
        },
      },
    })
    const form = wrapper.findComponent(ParamsForm)

    await formRow(wrapper, 'items').find('textarea.json-input').setValue('["new", 0, false]')
    await formRow(wrapper, 'options').find('textarea.json-input').setValue('{"enabled": true, "limit": 0}')
    await formRow(wrapper, 'value').find('textarea.json-input').setValue('false')
    expect(form.vm.getArgs()).toEqual({
      items: ['new', 0, false],
      options: { enabled: true, limit: 0 },
      value: false,
    })

    await formRow(wrapper, 'value').find('textarea.json-input').setValue('null')
    expect(form.vm.getArgs().value).toBeNull()
    await formRow(wrapper, 'value').find('textarea.json-input').setValue('"text"')
    expect(form.vm.getArgs().value).toBe('text')
  })

  it('JSON 语法/容器类型错误不会把文本写入 args，并可在修正后恢复', async () => {
    const wrapper = mount(ParamsForm, {
      props: { params: [decl('items', 'list', null, true), decl('options', 'object', null, true)] },
    })
    const form = wrapper.findComponent(ParamsForm)
    const items = formRow(wrapper, 'items').find('textarea.json-input')

    await items.setValue('{"not": "a list"}')
    expect(form.vm.getArgs().items).toBeNull()
    expect(form.vm.validate().map((error) => error.name)).toContain('items')
    expect(formRow(wrapper, 'items').text()).toContain('JSON 数组')

    await items.setValue('[1, 2]')
    expect(form.vm.getArgs().items).toEqual([1, 2])
    expect(form.vm.validate().map((error) => error.name)).not.toContain('items')
  })

  it('空数字保持 null，不会被输入控件伪造成 0；数字与布尔仍为真实类型', async () => {
    const wrapper = mount(ParamsForm, {
      props: {
        params: [decl('count', 'integer', null, true), decl('ratio', 'number', null, true), decl('flag', 'boolean', null, true)],
      },
    })
    const form = wrapper.findComponent(ParamsForm)
    expect(form.vm.getArgs()).toEqual({ count: null, ratio: null, flag: null })

    await formRow(wrapper, 'count').find('input[type="number"]').setValue('3')
    await formRow(wrapper, 'ratio').find('input[type="number"]').setValue('0')
    await formRow(wrapper, 'flag').find('select.cell-select').setValue('false')
    expect(form.vm.getArgs()).toEqual({ count: 3, ratio: 0, flag: false })

    await formRow(wrapper, 'ratio').find('input[type="number"]').setValue('')
    expect(form.vm.getArgs().ratio).toBeNull()
  })

  it('ParamEditor 默认值 JSON 编辑后可经 YAML 序列化/解析往返', async () => {
    const created = setupScript([
      'params:',
      '  items:',
      '    type: list',
      '    default: [old]',
      '  options:',
      '    type: object',
      '    default: {enabled: false}',
      '  empty:',
      '    type: any',
      '    default: null',
      'run: []',
    ].join('\n'))
    const wrapper = mount(ParamEditor, { props: { model: created.model, stack: created.stack } })
    await wrapper.find('button[title="展开参数列表"]').trigger('click')

    await wrapper.findAll('.param-row')[0].find('textarea.json-input').setValue('["new", 0]')
    await wrapper.findAll('.param-row')[1].find('textarea.json-input').setValue('{"enabled": true, "limit": 0}')
    await wrapper.findAll('.param-row')[2].find('textarea.json-input').setValue('null')
    expect(created.model.params.map((param) => param.default)).toEqual([
      ['new', 0],
      { enabled: true, limit: 0 },
      null,
    ])
    expect(created.model.params[2].hasDefault).toBe(true)

    const roundTripped = parseScript(serialize(created.model)).model
    expect(roundTripped.params.map((param) => param.default)).toEqual(created.model.params.map((param) => param.default))
    expect(roundTripped.params[2].hasDefault).toBe(true)
  })
})
