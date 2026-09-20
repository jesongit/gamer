// @vitest-environment happy-dom
import { afterEach, expect, it } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import StepCard from '../components/StepCard.vue'
import { SE_TARGET_OPTIONS } from '../targets'
import { serialize } from '../codec'
import { validateSource } from '../validation'
import { setupScript, expandCard } from './component_helpers'

const schema = [
  { name: 'text', type: 'string', required: true, default: null },
  { name: 'region', type: 'list', required: false, default: null },
  { name: 'flag', type: 'boolean', required: false, default: false },
  { name: 'count', type: 'integer', required: false, default: 0 },
  { name: 'empty', type: 'string', required: false, default: '' },
  { name: 'nullable', type: 'any', required: false, default: null, hasDefault: true },
  { name: 'options', type: 'object', required: false, default: { enabled: true } },
]
let wrapper
afterEach(() => wrapper?.unmount())
async function setup(source = 'run:\n  - demo: {text: hello}\n', group = 'plugin') {
  const created = setupScript(source)
  wrapper = mount(StepCard, {
    props: { ...created, step: created.model.run[0], containerPath: ['run'], basePath: 'run', index: 0 },
    global: { provide: { [SE_TARGET_OPTIONS]: {
      targets: [{ target: 'demo', group }], resolveParams: async () => schema, resolveParamsSync: () => schema,
    } } },
  })
  await expandCard(wrapper, created.model.run[0].uuid)
  return created
}

it.each(['plugin', 'package'])('%s 函数仅展开无默认值的必填参数，默认按钮保留 false/0/空串/null', async group => {
  const { model } = await setup(undefined, group)
  expect(wrapper.findAll('[data-arg-name]').map(w => w.attributes('data-arg-name'))).toEqual(['text'])
  expect(wrapper.get('[data-arg-name="text"]').find('[aria-label="必填"]').exists()).toBe(true)
  expect(wrapper.find('[data-param="text"]').exists()).toBe(false)
  expect(wrapper.get('.optional-params').attributes('open')).toBeUndefined()
  expect(wrapper.get('[data-param="region"]').classes()).not.toContain('needs-value')
  expect(wrapper.get('[data-param="region"]').attributes('aria-pressed')).toBe('false')
  expect(wrapper.find('[data-arg-name="region"]').exists()).toBe(false)
  await wrapper.get('[data-param="region"]').trigger('click')
  expect(wrapper.get('[data-arg-name="region"]').text()).toContain('可选，留空不传')
  expect(wrapper.get('[data-arg-name="region"]').find('.cell-error').exists()).toBe(false)
  expect(wrapper.get('textarea[aria-label="参数 region"]').element.value).toBe('')
  for (const [name, value] of [['flag', 'false'], ['count', '0'], ['empty', '""'], ['nullable', 'null']]) {
    expect(wrapper.get(`[data-param="${name}"]`).text()).toContain(`= ${value}`)
    expect(wrapper.get(`[data-param="${name}"]`).attributes('aria-pressed')).toBe('false')
  }
  expect(serialize(model)).not.toContain('region:')
  expect(validateSource(serialize(model), 'script', { resolveParams: () => schema }).diagnostics).toEqual([])
})

it('点击默认参数才加入实参，编辑、撤销、恢复默认和重开保持一致', async () => {
  const { model, stack } = await setup()
  await wrapper.get('[data-param="count"]').trigger('click')
  expect(model.run[0].args.entries.count).toEqual({ lit: 0 })
  await wrapper.get('input[aria-label="参数 count"]').setValue('7')
  const saved = serialize(model)
  await wrapper.get('[aria-label="恢复 count 默认值"]').trigger('click')
  expect(serialize(model)).not.toContain('count:')
  expect(wrapper.find('[data-arg-name="count"]').exists()).toBe(false)
  stack.undo()
  await nextTick()
  expect(wrapper.get('input[aria-label="参数 count"]').element.value).toBe('7')
  wrapper.unmount()
  await setup(saved)
  expect(wrapper.get('[data-param="count"]').attributes('aria-pressed')).toBe('true')
  expect(wrapper.get('input[aria-label="参数 count"]').element.value).toBe('7')
  expect(wrapper.find('[data-arg-name="flag"]').exists()).toBe(false)
})

it('无默认值的可选参数点击才展开，空白不保存，填写后重开保留并可取消', async () => {
  const { model, stack } = await setup()
  const original = serialize(model)
  await wrapper.get('[data-param="region"]').trigger('click')
  expect(serialize(model)).toBe(original)
  await wrapper.get('[aria-label="删除参数 region"]').trigger('click')
  expect(wrapper.find('[data-arg-name="region"]').exists()).toBe(false)
  await wrapper.get('[data-param="region"]').trigger('click')
  await wrapper.get('textarea[aria-label="参数 region"]').setValue('[0,0,1,1]')
  const saved = serialize(model)
  await wrapper.get('[data-param="region"]').trigger('click')
  expect(serialize(model)).toBe(original)
  expect(wrapper.find('[data-arg-name="region"]').exists()).toBe(false)
  stack.undo()
  await nextTick()
  expect(wrapper.get('textarea[aria-label="参数 region"]').element.value).toContain('1')
  wrapper.unmount()
  await setup(saved)
  expect(wrapper.get('[data-param="region"]').attributes('aria-pressed')).toBe('true')
  expect(wrapper.get('textarea[aria-label="参数 region"]').element.value).toContain('1')
})

it('对象默认值点击后独立复制，不改变函数声明', async () => {
  const { model } = await setup()
  await wrapper.get('[data-param="options"]').trigger('click')
  expect(model.run[0].args.entries.options.lit).not.toBe(schema[6].default)
  await wrapper.get('textarea[aria-label="参数 options"]').setValue('{"enabled":false}')
  expect(schema[6].default).toEqual({ enabled: true })
  await wrapper.get('[data-param="options"]').trigger('click')
  await wrapper.get('[data-param="options"]').trigger('click')
  expect(model.run[0].args.entries.options.lit).toEqual({ enabled: true })
})

it('选择新函数时默认参数不写入 YAML，必填空值提示填写', async () => {
  const { model } = await setup('run:\n  - old: {}\n')
  await wrapper.get('select[aria-label="函数"]').setValue('demo')
  await flushPromises()
  expect(model.run[0].args.entries).toEqual({ text: { lit: null, missing: true } })
  expect(wrapper.find('[data-arg-name="flag"]').exists()).toBe(false)
  expect(wrapper.get('[data-arg-name="text"]').find('.cell-error').exists()).toBe(true)
})
