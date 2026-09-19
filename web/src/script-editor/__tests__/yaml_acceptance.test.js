// @vitest-environment happy-dom
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { serialize } from '../codec'
import { hasParamDefault } from '../schema'
import { childStepLists, PARAM_TYPES } from '../model'
import { validateSource } from '../validation'
import { SE_TARGET_OPTIONS } from '../targets'
import StepCard from '../components/StepCard.vue'
import { setupScript, expandCard } from './component_helpers'

const fixture = file => readFileSync(resolve(process.cwd(), '../tools/yaml-tests', file), 'utf8')
const catalog = JSON.parse(fixture('native-functions.json'))
const ctx = {
  knownFunctions: new Set([...catalog.map(f => f.name), 'echo_value', 'nested_echo', 'early_return']),
  resolveParams: name => catalog.find(f => f.name === name)?.params,
  resolveTemplate: name => name === 'button.png',
}
const provide = { [SE_TARGET_OPTIONS]: {
  targets: catalog.map(f => ({ target: f.name, group: 'plugin', label: f.name })),
  resolveParamsSync: ctx.resolveParams,
  resolveParams: async name => ctx.resolveParams(name),
} }

describe('YAML 跨前后端验收夹具', () => {
  for (const file of ['_function.yaml', 'flow.yaml', 'native.yaml']) {
    it(`${file}：真实目录与模板校验、序列化往返`, () => {
      const kind = file.startsWith('_function') ? 'function_library' : 'script'
      const first = validateSource(fixture(file), kind, ctx)
      expect(first.diagnostics).toEqual([])
      const text = serialize(first.result.model)
      const second = validateSource(text, kind, ctx)
      expect(second.diagnostics).toEqual([])
      expect(serialize(second.result.model)).toBe(text)
    })
  }

  function tutorialExamples() {
    const tutorial = readFileSync(resolve(process.cwd(), '../docs/guides/yaml-tutorial.md'), 'utf8')
    const examples = [...tutorial.matchAll(/```yaml\r?\n([\s\S]*?)```/g)]
    expect(examples.length).toBeGreaterThan(0)
    return examples.flatMap(([, original]) => [original, original.replace(/^(\s*)# ([a-z_]\w*:.*)$/gm, '$1$2')])
      .map(source => ({ source, kind: source.trimStart().startsWith('functions:') ? 'function_library' : 'script' }))
  }

  it('教程全部案例及启用可选参数后的案例按真实函数目录校验和往返', () => {
    const examples = tutorialExamples()
    const functions = examples.filter(e => e.kind === 'function_library')
      .flatMap(e => validateSource(e.source, e.kind).result.model.functions)
    const tutorialCtx = {
      knownFunctions: new Set([...catalog, ...functions].map(f => f.name)),
      resolveParams: name => [...catalog, ...functions].find(f => f.name === name)?.params,
      resolveTemplate: name => name === '指南.png',
    }
    for (const { source, kind } of examples) {
      const first = validateSource(source, kind, tutorialCtx)
      expect(first.diagnostics, source).toEqual([])
      const text = serialize(first.result.model)
      const second = validateSource(text, kind, tutorialCtx)
      expect(second.diagnostics, source).toEqual([])
      expect(serialize(second.result.model), source).toBe(text)
    }
  })

  it('教程案例覆盖完整原生函数目录、每个参数及四种步骤', () => {
    const calls = new Map()
    const kinds = new Set()
    function visit(steps) {
      for (const step of steps) {
        kinds.add(step.kind)
        if (step.kind === 'call') {
          const params = calls.get(step.fn) || new Set()
          if (step.args.kind === 'map') Object.keys(step.args.entries).forEach(name => params.add(name))
          if (step.args.kind === 'value') params.add(catalog.find(f => f.name === step.fn)?.params[0]?.name)
          calls.set(step.fn, params)
        }
        for (const child of childStepLists(step)) visit(child.list)
      }
    }
    for (const { source, kind } of tutorialExamples()) {
      const first = validateSource(source, kind)
      expect(first.diagnostics, source).toEqual([])
      const model = first.result.model
      if (kind === 'function_library') model.functions.forEach(fn => visit(fn.run))
      else visit(model.run)
    }
    expect([...kinds].sort()).toEqual(['call', 'if', 'repeat', 'return'])
    for (const fn of catalog) {
      expect(calls.has(fn.name), `教程遗漏函数 ${fn.name}`).toBe(true)
      expect([...calls.get(fn.name)].sort(), `教程遗漏 ${fn.name} 参数`).toEqual(fn.params.map(p => p.name).sort())
    }
  })

  it('教程列出全部参数类型', () => {
    const tutorial = readFileSync(resolve(process.cwd(), '../docs/guides/yaml-tutorial.md'), 'utf8')
    for (const type of PARAM_TYPES) expect(tutorial).toContain(`| \`${type}\` |`)
  })

  it('文本、时间、返回字面量不误报模板缺失，模板参数仍校验', () => {
    const source = 'run:\n  - log: hello\n  - sleep: 1s\n  - if: null\n    then: []\n  - return: done\n'
    expect(validateSource(source, 'script', { ...ctx, resolveTemplate: () => false }).diagnostics).toEqual([])
    expect(validateSource('run:\n  - wait_find: missing\n', 'script', ctx).diagnostics.map(d => d.code))
      .toEqual(['yaml.resource.tmpl_not_found'])
    expect(validateSource('run:\n  - return: {items: [$missing, $$literal]}\n', 'script', ctx).diagnostics.map(d => d.code))
      .toEqual(['yaml.var.undefined'])
    expect(validateSource('run:\n  - custom: hello\n', 'script', {
      resolveTemplate: () => false,
      resolveParams: () => [{ name: 'text', type: 'string' }, { name: 'template', type: 'template' }],
    }).diagnostics).toEqual([])
  })
})

describe('全部 18 个内置函数的真实参数 Schema 表单', () => {
  for (const fn of catalog) {
    it(`${fn.name}：参数名称是纯文本，添加只能使用声明的参数`, async () => {
      const created = setupScript(`run:\n  - ${fn.name}: {}\n`)
      const wrapper = mount(StepCard, { props: { ...created, step: created.model.run[0], containerPath: ['run'], basePath: 'run', index: 0 }, global: { provide } })
      await expandCard(wrapper, created.model.run[0].uuid)
      expect(wrapper.findAll('.param-button')).toHaveLength(fn.params.filter(p => !p.required).length)
      expect(wrapper.findAll('.arg-name').map(w => w.text())).toEqual(fn.params.filter(p => p.required).map(p => p.name))
      for (const param of fn.params.filter(p => !p.required)) {
        const button = wrapper.get(`button[data-param="${param.name}"]`)
        expect(button.attributes('aria-pressed')).toBe(String(param.required && !hasParamDefault(param)))
        if (!param.required || hasParamDefault(param)) await button.trigger('click')
      }
      expect(wrapper.findAll('.arg-name').map(w => w.text()).sort()).toEqual(fn.params.map(p => p.name).sort())
      expect(wrapper.findAll('input[aria-label^="参数名"]')).toHaveLength(0)
      expect(wrapper.findAll('input[readonly]')).toHaveLength(0)
      expect(Object.keys(created.model.run[0].args.entries || {}).sort()).toEqual(fn.params.filter(p => !p.required && hasParamDefault(p)).map(p => p.name).sort())
      wrapper.unmount()
    })
  }

  it('位置简写使用参数类型控件，补默认参数保留用户输入', async () => {
    const created = setupScript('run:\n  - wait_find: button\n')
    const wrapper = mount(StepCard, { props: { ...created, step: created.model.run[0], containerPath: ['run'], basePath: 'run', index: 0 }, global: { provide } })
    await expandCard(wrapper, created.model.run[0].uuid)
    await wrapper.get('button[data-param="timeout"]').trigger('click')
    expect(created.model.run[0].args.entries.template).toEqual({ lit: 'button' })
    await wrapper.get('input[aria-label="参数 timeout数值"]').setValue('5')
    expect(created.model.run[0].args.entries.timeout).toEqual({ lit: '5s' })
    wrapper.unmount()
  })
})
