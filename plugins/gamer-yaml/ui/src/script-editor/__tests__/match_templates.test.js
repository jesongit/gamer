// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import { parseScript, serialize } from '../codec'
import { validateSource } from '../validation'
import { childStepLists, cloneStepWithNewUuids } from '../model'
import { resolveStepList } from '../commands'
import { findStepLocation } from '../selection'
import { locateDiagnostic } from '../components/kinds'
import StepCard from '../components/StepCard.vue'
import { setupScript } from './component_helpers'

const source = `run:
  - match_templates:
      cases:
        - template: notice.png
          as: hit
          do:
            - tap: $hit.center
        - template: login.png
          do:
            - log: login
      else:
        - log: missing
`

describe('模板分支：完整编辑与执行路径契约', () => {
  it('保存重开、分支局部变量、嵌套定位和复制', () => {
    const first = validateSource(source, 'script', { knownFunctions: new Set(['tap', 'log']), resolveTemplate: () => true })
    expect(first.diagnostics).toEqual([])
    const model = first.result.model
    const text = serialize(model)
    expect(validateSource(text, 'script').diagnostics).toEqual([])
    expect(serialize(parseScript(text).model)).toBe(text)
    const child = model.run[0].cases[0].body[0]
    const location = findStepLocation(model, child.uuid)
    expect(location.stepPath).toBe('run[0].cases[0].do[0]')
    expect(resolveStepList(model, location.containerPath)[0]).toBe(child)
    expect(locateDiagnostic(model, { step_path: location.stepPath }).uuid).toBe(child.uuid)
    const clone = cloneStepWithNewUuids(model.run[0])
    expect(childStepLists(clone)).toHaveLength(3)
    expect(clone.cases[0].body[0].uuid).not.toBe(child.uuid)
    expect(validateSource(source + '  - tap: $hit.center\n', 'script').diagnostics.some(d => d.message.includes('hit'))).toBe(true)
    expect(validateSource(source.replace('log: login', 'tap: $hit.center'), 'script').diagnostics.some(d => d.message.includes('hit'))).toBe(true)
  })

  it('模板选择、分支新增与排序、子步骤撤销重做保持对象关联', async () => {
    const { model, stack } = setupScript(source)
    const step = model.run[0]
    const wrapper = mount(StepCard, { props: {
      model, stack, step, index: 0, containerPath: ['run'], basePath: 'run',
      expandedUuids: new Set([step.uuid]), templates: ['notice.png', 'login.png', 'home.png'],
    } })
    expect(wrapper.findAll('.template-case')).toHaveLength(2)
    expect(wrapper.findAll('.template-case .tpl-toggle')).toHaveLength(2)
    await wrapper.get('input[aria-label="分支 2 模板"]').setValue('home.png')
    const child = step.cases[0].body[0]
    stack.apply({ type: 'update_step', path: ['run', 0, 'cases[0].do', 0], fields: { fn: 'log' } })
    await wrapper.get('button[aria-label="下移模板分支 1"]').trigger('click')
    expect(step.cases[1].body[0]).toBe(child)
    stack.undo()
    stack.undo()
    expect(step.cases[0].body[0].fn).toBe('tap')
    stack.redo()
    stack.redo()
    expect(step.cases[1].body[0].fn).toBe('log')
    await nextTick()
    await wrapper.get('button[aria-label="添加模板分支"]').trigger('click')
    expect(step.cases).toHaveLength(3)
    await wrapper.get('input[aria-label="分支 3 模板"]').setValue('login.png')
    await wrapper.get('input[aria-label="分支 3 匹配结果"]').setValue('login_hit')
    const reopened = parseScript(serialize(model)).model
    expect(reopened.run[0].cases[2]).toMatchObject({ template: { lit: 'login.png' }, as: 'login_hit', body: [] })
    await wrapper.get('button[aria-label="删除模板分支 3"]').trigger('click')
    expect(step.cases).toHaveLength(2)
    stack.undo()
    expect(step.cases[2].as).toBe('login_hit')
  })

  it.each([
    'cases: []',
    'cases: [{template: "", do: []}]',
    'cases: [{template: a, do: [], unknown: 1}]',
    'cases: [{template: a}]',
    'cases: [{template: a, as: 123, do: []}]',
    'cases: [{template: a, do: []}]\n      threshold: 2',
  ])('拒绝无效分支 %s', body => {
    expect(validateSource(`run:\n  - match_templates:\n      ${body}\n`, 'script').diagnostics.length).toBeGreaterThan(0)
  })
})
