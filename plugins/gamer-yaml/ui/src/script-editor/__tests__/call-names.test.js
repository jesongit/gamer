// @vitest-environment happy-dom
import { expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { parseScript, serialize } from '../codec'
import { validateSource } from '../validation'
import { functionCallParams, NATIVE_CALL_NAMES } from '../call-names'
import { stepSummary } from '../components/kinds'
import { SE_TARGET_OPTIONS } from '../targets'
import StepCard from '../components/StepCard.vue'
import ScriptSummary from '../../components/console/ScriptSummary.vue'
import { setupScript, expandCard } from './component_helpers'
import catalog from '../../../../../../tools/yaml-tests/native-functions.json'

it('全部内置函数的中文默认名与服务端目录一致，timeout 默认为 3s', () => {
  for (const fn of catalog) {
    expect(NATIVE_CALL_NAMES[fn.name]).toBe(fn.params.find(p => p.name === 'name').default)
    expect(stepSummary(parseScript(`run:\n  - ${fn.name}: {}\n`).model.run[0])).toBe(NATIVE_CALL_NAMES[fn.name])
    for (const param of fn.params.filter(p => p.name === 'timeout')) expect(param.default).toBe('3s')
  }
})

it('name 参数可编辑、往返，编辑卡片与只读摘要只显示其值', async () => {
  const created = setupScript('run:\n  - tap: {position: [0.5, 0.8], name: 点击登录}\n')
  const schema = catalog.find(f => f.name === 'tap').params
  const wrapper = mount(StepCard, { props: {
    step: created.model.run[0], model: created.model, stack: created.stack, containerPath: ['run'], basePath: 'run', index: 0,
  }, global: { provide: { [SE_TARGET_OPTIONS]: {
    targets: [{ target: 'tap', group: 'plugin' }], resolveParamsSync: () => schema, resolveParams: async () => schema,
  } } } })
  expect(wrapper.get('.summary').text()).toBe('点击登录')
  await expandCard(wrapper, created.model.run[0].uuid)
  await wrapper.get('input[aria-label="参数 name"]').setValue('确认登录')
  const reparsed = parseScript(serialize(created.model))
  expect(reparsed.model.run[0].args.entries.name).toEqual({ lit: '确认登录' })
  expect(reparsed.model.run[0].fn).toBe('tap')
  const summary = mount(ScriptSummary, { props: { model: reparsed.model } })
  expect(summary.get('.summary').text()).toBe('确认登录')
  wrapper.unmount()
  summary.unmount()
})

it('配置包函数有通用 name 默认值，并保留已有声明和位置参数', () => {
  const original = [{ name: 'value', type: 'string', required: true, default: null, desc: '' }]
  const params = functionCallParams({ name: 'claim', description: '领取奖励', params: original })
  expect(params.map(p => p.name)).toEqual(['value', 'name'])
  expect(params[1].default).toBe('领取奖励')
  expect(original).toHaveLength(1)
  expect(functionCallParams({ name: '领取奖励' })[0].default).toBe('领取奖励')
  expect(functionCallParams({ name: 'claim', params })).toBe(params)
})

it('name 支持引用，拒绝非字符串字面量', () => {
  expect(validateSource('vars: {label: 点击登录}\nrun:\n  - tap: {name: $label, position: [0.5, 0.8]}\n', 'script').diagnostics).toEqual([])
  expect(validateSource('run:\n  - log: {name: 123, message: hello}\n', 'script').diagnostics.map(d => d.code)).toContain('yaml.args.type')
})
