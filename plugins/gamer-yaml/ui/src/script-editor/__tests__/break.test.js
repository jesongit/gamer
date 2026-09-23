// @vitest-environment happy-dom
import { expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { parseScript, parseFunctionLibrary, serialize } from '../codec'
import { validateScript, validateFunctionLibrary } from '../validation'
import { createControl } from '../factories'
import { CommandStack } from '../commands'
import StepCanvas from '../components/StepCanvas.vue'

it('break 往返保留，if/模板分支继承循环作用域，函数作用域独立', () => {
  const source = `run:
  - repeat: 3
    do:
      - if: true
        then:
          - break: {}
      - match_templates:
          cases:
            - template: a.png
              do:
                - break: {}
          else:
            - break: {}
  - log: done
`
  const parsed = parseScript(source)
  expect(parsed.diagnostics).toEqual([])
  expect(validateScript(parsed.model)).toEqual([])
  expect(serialize(parseScript(serialize(parsed.model)).model)).toBe(serialize(parsed.model))
  const outside = parseScript('run: [{if: true, then: [{break: {}}]}]')
  expect(validateScript(outside.model)).toEqual([expect.objectContaining({ code: 'yaml.break.outside_loop', step_path: 'run[0].then[0]' })])
  const functions = parseFunctionLibrary('functions: {helper: {run: [{break: {}}]}}')
  expect(validateFunctionLibrary(functions.model)[0].code).toBe('yaml.break.outside_loop')
  for (const value of ['true', '1', '{value: 1}']) {
    expect(parseScript(`run: [{repeat: 1, do: [{break: ${value}}]}]`).diagnostics[0].code).toBe('yaml.break.shape')
  }
  expect(parseScript('run: [{break: {}, as: hit}]').diagnostics[0].code).toBe('yaml.as.invalid')
})

it('画布可添加 break，移动到循环外会给诊断，撤销后恢复合法', async () => {
  const model = parseScript('run: [{repeat: 3, do: []}]').model
  const stack = new CommandStack(model)
  const wrapper = mount(StepCanvas, { props: { model, stack, diagnostics: [], compactToolbar: true }, global: { stubs: { Teleport: true } } })
  await wrapper.get('.add-btn').trigger('click')
  expect(wrapper.get('[data-kind="break"]').text()).toBe('跳出循环')
  await wrapper.get('[data-kind="break"]').trigger('click')
  expect(validateScript(model)[0].code).toBe('yaml.break.outside_loop')
  stack.undo()
  stack.apply({ type: 'insert_step', path: ['run', 0, 'body'], index: 0, step: createControl('break') })
  expect(validateScript(model)).toEqual([])
  expect(serialize(model)).toContain('break: {}')
  wrapper.unmount()
})
